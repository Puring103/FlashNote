"""Local Casdoor protocol fixture; never used by the deployed service."""
import json
from pathlib import Path
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs
import uuid


class ProviderHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        values = parse_qs(self.rfile.read(int(self.headers['Content-Length'])).decode())
        fixture = self.server.fixture
        with fixture.lock:
            if values.get('client_secret') != ['fixture-secret']:
                value = {'error': 'invalid_client'}
            else:
                value = fixture.dispatch(self.path.rsplit('/', 1)[-1], values)
            fixture.save()
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Fixture:
    def __init__(self, port=0, state_path=None):
        self.state_path = Path(state_path) if state_path else None
        self.tokens = json.loads(self.state_path.read_text()) if self.state_path and self.state_path.exists() else {}
        self.lock = threading.RLock()
        self.server = ThreadingHTTPServer(('127.0.0.1', port), ProviderHandler)
        self.server.daemon_threads = True
        self.server.fixture = self
        self.issuer = f'http://127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def save(self):
        if self.state_path:
            self.state_path.write_text(json.dumps(self.tokens))

    def token(self, user='alice', **claims):
        with self.lock:
            token = str(uuid.uuid4())
            self.tokens[token] = {'active': True, 'iss': self.issuer, 'sub': user,
                                 'username': user, 'aud': ['suojian-test'],
                                 'client_id': 'suojian-test', 'exp': time.time()+900,
                                 **claims}
            self.save()
            return token

    def issue(self, user):
        access = self.token(user)
        refresh = str(uuid.uuid4())
        self.tokens[refresh] = {'user': user, 'access': access}
        return {'access_token': access, 'refresh_token': refresh,
                'token_type': 'Bearer', 'expires_in': 900}

    def dispatch(self, action, values):
        if action == 'introspect':
            return self.tokens.get(values.get('token', [''])[0], {'active': False})
        if action == 'revoke':
            record = self.tokens.pop(values.get('token', [''])[0], {})
            self.tokens.pop(record.get('access'), None)
            return {}
        if action == 'access_token':
            if values.get('grant_type') == ['refresh_token']:
                record = self.tokens.pop(values.get('refresh_token', [''])[0], {})
                if 'user' not in record:
                    return {'error': 'invalid_grant'}
                self.tokens.pop(record.get('access'), None)
                return self.issue(record['user'])
            user = values.get('username', [''])[0]
            if user not in ('alice', 'bob') or values.get('password') != ['login123']:
                return {'error': 'invalid_grant'}
            return self.issue(user)
        return {'error': 'unsupported_action'}

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
