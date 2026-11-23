"""Server-side Casdoor adapter. Client secrets never leave the sync server."""
import json
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen

from identity import Identity, IdentityUnavailable


class Casdoor:
    def __init__(self, endpoint, issuer, client_id, client_secret, database=None):
        for value in (endpoint, issuer):
            parsed = urlsplit(value)
            if (parsed.scheme != 'https' and not (parsed.scheme == 'http' and parsed.hostname in ('localhost', '127.0.0.1', '::1'))
                    or parsed.username or parsed.password or parsed.query or parsed.fragment):
                raise ValueError('Casdoor requires HTTPS (HTTP only on loopback)')
        if not client_id or not client_secret:
            raise ValueError('Casdoor client credentials are required')
        self.endpoint = endpoint.rstrip('/')
        self.issuer, self.client_id, self.client_secret = issuer, client_id, client_secret
        self.database = database
        if database:
            with database.connect() as db:
                db.execute('CREATE TABLE IF NOT EXISTS casdoor_attempts (name TEXT PRIMARY KEY, count INTEGER NOT NULL, until INTEGER NOT NULL)')

    def configuration(self):
        return {'issuer': self.issuer, 'client_id': self.client_id}

    def request(self, action, values):
        values = {**values, 'client_id': self.client_id, 'client_secret': self.client_secret}
        request = Request(self.endpoint + '/api/login/oauth/' + action,
                          data=urlencode(values).encode(),
                          headers={'Content-Type': 'application/x-www-form-urlencoded'})
        try:
            with urlopen(request, timeout=10) as response:
                body = response.read(65537)
            if len(body) > 65536:
                raise IdentityUnavailable('Casdoor response too large')
            value = json.loads(body) if body else {}
        except HTTPError as error:
            if error.code in (400, 401, 403):
                raise ValueError('invalid credentials or session') from None
            raise IdentityUnavailable('Casdoor unavailable') from None
        except (URLError, TimeoutError, OSError, json.JSONDecodeError):
            raise IdentityUnavailable('Casdoor unavailable') from None
        if not isinstance(value, dict):
            raise IdentityUnavailable('invalid Casdoor response')
        if value.get('error') or value.get('status') == 'error':
            raise ValueError('invalid credentials or session')
        return value

    def tokens(self, values):
        value = self.request('access_token', values)
        if (not isinstance(value.get('access_token'), str) or not value['access_token']
                or not isinstance(value.get('refresh_token'), str) or not value['refresh_token']
                or str(value.get('token_type', '')).lower() != 'bearer'
                or not isinstance(value.get('expires_in'), int)
                or not 30 <= value['expires_in'] <= 86400):
            raise IdentityUnavailable('invalid Casdoor token response')
        return {key: value[key] for key in ('access_token', 'refresh_token', 'token_type', 'expires_in')} | self.configuration()

    def login(self, username, secret, peer):
        if (not isinstance(username, str) or not username.strip() or len(username) > 128
                or not isinstance(secret, str) or not secret or len(secret) > 1024):
            raise ValueError('invalid credentials')
        # Count before forwarding, including errors/timeouts. Persistent limits prevent
        # password-grant calls from bypassing the interactive login page's protections.
        if self.database:
            now = int(time.time())
            with self.database.connect() as db:
                db.execute('BEGIN IMMEDIATE')
                db.execute('DELETE FROM casdoor_attempts WHERE until<=?', (now,))
                for key, limit in (('user:' + username.strip().casefold(), 10), ('peer:' + peer, 120)):
                    row = db.execute('SELECT count FROM casdoor_attempts WHERE name=?', (key,)).fetchone()
                    if row and row['count'] >= limit:
                        raise ValueError('login rate exceeded')
                for key in ('user:' + username.strip().casefold(), 'peer:' + peer):
                    db.execute('INSERT INTO casdoor_attempts VALUES (?,1,?) ON CONFLICT(name) DO UPDATE SET count=count+1', (key, now + 60))
        return self.tokens({'grant_type': 'password', 'username': username.strip(),
                            'password': secret, 'scope': 'openid profile email offline_access'})

    def refresh(self, token):
        self.validate_token(token)
        return self.tokens({'grant_type': 'refresh_token', 'refresh_token': token})

    def revoke(self, token):
        self.validate_token(token)
        self.request('revoke', {'token': token, 'token_type_hint': 'refresh_token'})

    @staticmethod
    def validate_token(token):
        if not isinstance(token, str) or not token or len(token) > 16384:
            raise ValueError('invalid token')

    def verify(self, authorization):
        if not authorization.startswith('Bearer '):
            raise ValueError('missing token')
        token = authorization[7:]
        self.validate_token(token)
        value = self.request('introspect', {'token': token, 'token_type_hint': 'access_token'})
        audience = value.get('aud')
        if isinstance(audience, str):
            audience = [audience]
        subject = value.get('sub')
        if (value.get('active') is not True or value.get('client_id') != self.client_id
                or value.get('iss') != self.issuer or not isinstance(audience, list)
                or self.client_id not in audience or not isinstance(subject, str) or not subject
                or not isinstance(value.get('exp'), (int, float)) or value['exp'] <= time.time()):
            raise ValueError('inactive or wrong-application token')
        name = value.get('username') or subject
        if not isinstance(name, str) or len(name) > 512:
            raise ValueError('invalid name')
        return Identity(self.issuer, subject, name)
