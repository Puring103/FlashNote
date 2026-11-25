"""Casdoor HTTP contract: application isolation, outages, rotation and logout."""
import json
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs
from unittest.mock import patch

from bootstrap import bootstrap
from casdoor import Casdoor
from identity import IdentityUnavailable
from server import Database


class ProviderHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = parse_qs(self.rfile.read(int(self.headers['Content-Length'])).decode())
        if body.get('client_secret') != ['server-only-secret']:
            self.send_response(401)
            self.end_headers()
            return
        action = self.path.rsplit('/', 1)[-1]
        if self.server.unavailable:
            self.send_response(503)
            self.end_headers()
            return
        if action == 'introspect':
            value = {'active': self.server.active, 'iss': self.server.origin,
                     'sub': 'stable-user-id', 'username': 'alice', 'aud': ['suojian'],
                     'client_id': self.server.client_id, 'exp': time.time() + 900}
        elif action == 'revoke':
            self.server.active = False
            value = {}
        elif body.get('grant_type') == ['refresh_token']:
            if body.get('refresh_token') != ['refresh-first']:
                value = {'error': 'invalid_grant'}
            else:
                value = {'access_token': 'access-second', 'refresh_token': 'refresh-second',
                         'token_type': 'Bearer', 'expires_in': 900}
        elif body.get('password') != ['correct-password']:
            value = {'error': 'invalid_grant'}
        else:
            value = {'access_token': 'access-first', 'refresh_token': 'refresh-first',
                     'token_type': 'Bearer', 'expires_in': 900}
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(value).encode())


class RegistrationTests(unittest.TestCase):
    @patch("bootstrap.os.chown", create=True)
    def test_registration_toggle_reconfigures_existing_database_and_proxy(self, _chown):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            env = {"SUOJIAN_ORIGIN": "https://notes.example"}
            bootstrap(root, env)
            seed = root / "casdoor-runtime" / "init_data.json"
            account = (root / "initial-account.json").read_bytes()
            self.assertFalse(json.loads(seed.read_text(encoding="utf8"))["applications"][0]["enableSignUp"])
            with sqlite3.connect(root / "data" / "casdoor.db") as db:
                db.execute("CREATE TABLE application(owner, client_id, enable_sign_up, signup_items, display_name)")
                db.execute("CREATE TABLE organization(owner,name,display_name)")
                db.execute("CREATE TABLE user(owner,name,display_name)")
                db.execute("INSERT INTO application VALUES('admin','suojian',0,NULL,'索笺')")
                db.commit()
                for enabled in (True, False):
                    bootstrap(root, env | {"SUOJIAN_REGISTRATION": str(enabled).lower()})
                    app = json.loads(seed.read_text(encoding="utf8"))["applications"][0]
                    self.assertEqual(app["enableSignUp"], enabled)
                    self.assertEqual(bool(db.execute("SELECT enable_sign_up FROM application").fetchone()[0]), enabled)
                    self.assertEqual("handle @signup" in (root / "Caddyfile").read_text(), not enabled)
            db.close()
            self.assertEqual((root / "initial-account.json").read_bytes(), account)


class CasdoorTests(unittest.TestCase):
    def setUp(self):
        self.provider = ThreadingHTTPServer(('127.0.0.1', 0), ProviderHandler)
        self.provider.origin = f'http://127.0.0.1:{self.provider.server_port}'
        self.provider.active, self.provider.unavailable = True, False
        self.provider.client_id = 'suojian'
        self.thread = threading.Thread(target=self.provider.serve_forever, daemon=True)
        self.thread.start()
        self.temp = tempfile.TemporaryDirectory()
        self.database = Database(Path(self.temp.name) / 'sync.sqlite3', 1024 * 1024)
        self.adapter = Casdoor(self.provider.origin, self.provider.origin, 'suojian',
                               'server-only-secret', self.database)

    def tearDown(self):
        self.provider.shutdown()
        self.provider.server_close()
        self.thread.join()
        self.temp.cleanup()

    def test_login_refresh_logout_and_wrong_application(self):
        first = self.adapter.login('alice', 'correct-password', 'local')
        self.assertNotIn('client_secret', first)
        self.assertEqual(self.adapter.verify('Bearer ' + first['access_token']).subject, 'stable-user-id')
        self.provider.client_id = 'another-app'
        with self.assertRaises(ValueError):
            self.adapter.verify('Bearer ' + first['access_token'])
        self.provider.client_id = 'suojian'
        second = self.adapter.refresh(first['refresh_token'])
        self.assertNotEqual(first['refresh_token'], second['refresh_token'])
        self.adapter.revoke(second['refresh_token'])
        with self.assertRaises(ValueError):
            self.adapter.verify('Bearer ' + second['access_token'])

    def test_wrong_password_and_provider_outage(self):
        with self.assertRaises(ValueError):
            self.adapter.login('alice', 'wrong', 'local')
        self.provider.unavailable = True
        with self.assertRaises(IdentityUnavailable):
            self.adapter.login('alice', 'correct-password', 'local')

    def test_password_grant_rate_limit_survives_adapter_restart(self):
        for _ in range(10):
            with self.assertRaises(ValueError):
                self.adapter.login('alice', 'wrong', 'local')
        restarted = Casdoor(self.provider.origin, self.provider.origin, 'suojian',
                            'server-only-secret', self.database)
        with patch.object(restarted, 'request') as upstream:
            with self.assertRaises(ValueError):
                restarted.login('ALICE', 'correct-password', 'other-peer')
            upstream.assert_not_called()

    def test_wrong_issuer_expiry_and_identity_metadata_are_rejected(self):
        valid = {'active': True, 'iss': self.provider.origin, 'client_id': 'suojian',
                 'aud': ['suojian'], 'sub': 'alice', 'exp': time.time() + 900}
        for overrides in ({'iss': 'https://other.example'}, {'exp': 0},
                          {'active': False}, {'aud': ['other-app']}, {'sub': ''}):
            with self.subTest(overrides=overrides), patch.object(self.adapter, 'request', return_value=valid | overrides):
                with self.assertRaises(ValueError):
                    self.adapter.verify('Bearer some-token')


if __name__ == '__main__':
    unittest.main()
