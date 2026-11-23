import base64
from concurrent.futures import ThreadPoolExecutor
import json
import sqlite3
from pathlib import Path
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from urllib.parse import urlencode
import uuid

from server import BROWSE_PAGE_BYTES, ClosingConnection, Database, Handler, ThreadingHTTPServer
from identity import Identity
from casdoor import Casdoor
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests/support"))
from casdoor_fixture import Fixture


def uid():
    return str(uuid.uuid4())


def encrypted(size=60):
    return b"\x01" + b"x" * (size - 1)


class SyncServerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.database = Database(Path(self.temp.name) / "sync.sqlite3", 1024 * 1024)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.database = self.database
        self.issuer = Fixture()
        self.server.accounts = Casdoor(self.issuer.issuer, self.issuer.issuer, "suojian-test", "fixture-secret", self.database)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.account = self.new_account("alice")

    def new_account(self, user):
        identity = Identity(self.issuer.issuer, user, user)
        account = self.database.managed_account(identity)
        return {"vault_id": account["vault_id"], "token": self.issuer.token(user)}

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.issuer.close()
        self.temp.cleanup()

    def request(self, method, route, payload=None, account=None):
        account = account or self.account
        if isinstance(payload, dict):
            payload = json.dumps(payload).encode()
        req = urllib.request.Request(self.url + f"/v1/{account['vault_id']}/{route}", data=payload, method=method,
                                     headers={"Authorization": "Bearer " + account["token"]})
        try:
            with urllib.request.urlopen(req, timeout=5) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()

    def packet(self, oid, base=None, assets=None, payload=None):
        return {"header": {"version": 1, "vault_id": self.account["vault_id"], "object_id": oid,
                "kind": "note", "revision": uid(), "base_revision": base, "device": uid(),
                "assets": sorted(assets or [])}, "ciphertext": base64.b64encode(payload or encrypted()).decode()}

    def raw(self, method, route, token=None, payload=None):
        request = urllib.request.Request(self.url + route, method=method,
            data=json.dumps(payload).encode() if payload is not None else None,
            headers={"Authorization": "Bearer " + token} if token else {})
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def test_invalid_tokens(self):
        for token in (None, "invalid-token", self.issuer.token(aud="another-app"),
                      self.issuer.token(exp=time.time()-10), self.issuer.token(iss="https://another-issuer/")):
            self.assertEqual(self.raw("POST", "/v1/account/key", token, {})[0], 401)
        self.assertEqual(self.raw("GET", "/auth/config")[1]["mode"], "casdoor")

    def test_account_identity_resolves_same_managed_key_concurrently(self):
        def create(_):
            return self.raw("POST", "/v1/account/key", self.issuer.token("bob"), {})
        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(create, range(8)))
        self.assertTrue(all(status == 200 for status, _ in results))
        self.assertEqual(len({a["vault_id"] for _, a in results}), 1)
        self.assertEqual(len({a["key"] for _, a in results}), 1)
        self.assertEqual(self.database.managed_account(Identity(self.issuer.issuer,"bob","renamed"))["name"], "renamed")

    def test_authentication_and_vault_isolation(self):
        aid = uid()
        self.assertEqual(self.request("PUT", "assets/" + aid, encrypted())[0], 200)
        other = self.new_account("bob")
        self.assertEqual(self.request("GET", "assets/" + aid, account=other)[0], 404)
        wrong = {"vault_id": self.account["vault_id"], "token": other["token"]}
        self.assertEqual(self.request("GET", "usage", account=wrong)[0], 401)

    def test_object_reads_return_latest_encrypted_envelope(self):
        for kind in ("note", "group"):
            with self.subTest(kind=kind):
                oid = uid()
                first = self.packet(oid)
                first["header"]["kind"] = kind
                self.assertEqual(self.request("PUT", "objects/" + oid, first)[0], 200)
                status, response = self.request("GET", "objects/" + oid)
                self.assertEqual(status, 200)
                self.assertEqual(json.loads(response), first)
                second = self.packet(oid, first["header"]["revision"], payload=encrypted(80))
                second["header"]["kind"] = kind
                self.assertEqual(self.request("PUT", "objects/" + oid, second)[0], 200)
                before = self.request("GET", "usage")
                self.assertEqual(json.loads(self.request("GET", "objects/" + oid)[1]), second)
                req = urllib.request.Request(self.url + f"/v1/{self.account['vault_id']}/objects/{oid}",
                    method="HEAD", headers={"Authorization": "Bearer " + self.account["token"]})
                with urllib.request.urlopen(req, timeout=5) as result:
                    self.assertEqual(result.status, 200)
                    self.assertEqual(result.read(), b"")
                    self.assertEqual(result.headers["Cache-Control"], "no-store")
                    self.assertEqual(int(result.headers["Content-Length"]),
                                     len(self.request("GET", "objects/" + oid)[1]))
                self.assertEqual(self.request("GET", "usage"), before)

    def test_object_reads_are_authenticated_and_vault_scoped(self):
        oid = uid()
        self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid))[0], 200)
        other = self.new_account("bob")
        wrong = {"vault_id": self.account["vault_id"], "token": other["token"]}
        for method in ("GET", "HEAD"):
            with self.subTest(method=method):
                self.assertEqual(self.request(method, "objects/" + oid, account=other)[0], 404)
                self.assertEqual(self.request(method, "objects/" + oid, account=wrong)[0], 401)
                self.assertEqual(self.request(method, "objects/" + uid())[0], 404)
                req = urllib.request.Request(self.url + f"/v1/{self.account['vault_id']}/objects/{oid}",
                                             method=method)
                with self.assertRaises(urllib.error.HTTPError) as error:
                    urllib.request.urlopen(req, timeout=5)
                self.assertEqual(error.exception.code, 401)

    def test_browsing_pages_are_partial_filtered_and_do_not_advance_sync(self):
        notes, groups = [], []
        for kind, ids, count in (("note", notes, 25), ("group", groups, 3)):
            for _ in range(count):
                oid = uid()
                packet = self.packet(oid)
                packet["header"]["kind"] = kind
                self.assertEqual(self.request("PUT", "objects/" + oid, packet)[0], 200)
                ids.append(oid)
        usage, changes = self.request("GET", "usage"), self.request("GET", "changes?after=0")
        for kind, ids in (("note", notes), ("group", groups)):
            cursor, collected = None, []
            while True:
                query = {"kind": kind, "limit": 7}
                if cursor:
                    query["cursor"] = cursor
                status, body = self.request("GET", "objects?" + urlencode(query))
                self.assertEqual(status, 200)
                page = json.loads(body)
                self.assertLessEqual(len(page["items"]), 7)
                self.assertTrue(all(item["header"]["kind"] == kind for item in page["items"]))
                collected.extend(item["header"]["object_id"] for item in page["items"])
                if not page["more"]:
                    self.assertIsNone(page["next_cursor"])
                    break
                self.assertTrue(page["next_cursor"])
                cursor = page["next_cursor"]
            self.assertEqual(collected, list(reversed(ids)))
        self.assertEqual(len(json.loads(self.request("GET", "objects")[1])["items"]), 20)
        self.assertEqual(self.request("GET", "usage"), usage)
        self.assertEqual(self.request("GET", "changes?after=0"), changes)

    def test_browsing_restarts_when_objects_change_between_pages(self):
        packets = []
        for _ in range(3):
            packet = self.packet(uid())
            packets.append(packet)
            self.assertEqual(self.request("PUT", "objects/" + packet["header"]["object_id"], packet)[0], 200)
        page = json.loads(self.request("GET", "objects?limit=1")[1])
        continuation = "objects?" + urlencode({"limit": 1, "cursor": page["next_cursor"]})
        # A lost-ack retry does not invalidate the browsing watermark.
        self.assertEqual(self.request("PUT", "objects/" + packets[0]["header"]["object_id"], packets[0])[0], 200)
        self.assertEqual(self.request("GET", continuation)[0], 200)
        edited = self.packet(packets[0]["header"]["object_id"], packets[0]["header"]["revision"])
        self.assertEqual(self.request("PUT", "objects/" + edited["header"]["object_id"], edited)[0], 200)
        self.assertEqual(self.request("GET", continuation)[0], 409)
        refreshed = json.loads(self.request("GET", "objects?limit=1")[1])
        self.assertEqual(refreshed["items"], [edited])

    def test_browsing_authentication_cursor_validation_and_empty_pages(self):
        empty = json.loads(self.request("GET", "objects?kind=note")[1])
        self.assertEqual(empty, {"items": [], "next_cursor": None, "more": False, "snapshot": 0})
        other = self.new_account("bob")
        wrong = {"vault_id": self.account["vault_id"], "token": other["token"]}
        self.assertEqual(self.request("GET", "objects", account=wrong)[0], 401)
        for _ in range(2):
            oid = uid()
            self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid))[0], 200)
        cursor = json.loads(self.request("GET", "objects?kind=note&limit=1")[1])["next_cursor"]
        route = "objects?" + urlencode({"kind": "note", "cursor": cursor})
        self.assertEqual(self.request("GET", route, account=other)[0], 400)
        self.assertEqual(self.request("GET", "objects?" + urlencode({"kind": "group", "cursor": cursor}))[0], 400)
        self.assertEqual(json.loads(self.request("GET", "objects?kind=note", account=other)[1])["items"], [])
        for query in ("limit=0", "limit=51", "limit=no", "limit=1.5", "limit=", "limit=1&limit=2",
                      "kind=unknown", "kind=", "cursor=bad!", "cursor=", "unknown=1"):
            with self.subTest(query=query):
                self.assertEqual(self.request("GET", "objects?" + query)[0], 400)
        for state in ([], {"version": 1}, {"version": 1, "vault": self.account["vault_id"], "kind": "note",
                       "snapshot": True, "before": 0}):
            malformed = base64.urlsafe_b64encode(json.dumps(state).encode()).decode()
            self.assertEqual(self.request("GET", "objects?" + urlencode({"kind": "note", "cursor": malformed}))[0], 400)

    def test_browsing_byte_budget_and_oversized_note_make_progress(self):
        expected = []
        for size in (60, BROWSE_PAGE_BYTES // 2, BROWSE_PAGE_BYTES):
            oid = uid()
            packet = self.packet(oid, payload=encrypted(size))
            self.assertEqual(self.request("PUT", "objects/" + oid, packet)[0], 200)
            expected.append(oid)
        first = json.loads(self.request("GET", "objects?limit=20")[1])
        self.assertEqual([item["header"]["object_id"] for item in first["items"]], [expected[-1]])
        self.assertTrue(first["more"])
        second = json.loads(self.request("GET", "objects?" + urlencode({"cursor": first["next_cursor"]}))[1])
        self.assertEqual([item["header"]["object_id"] for item in second["items"]], list(reversed(expected[:-1])))
        self.assertFalse(second["more"])
        # A page can be shortened by bytes even if its count is below the limit.
        oid = uid()
        self.assertEqual(self.request("PUT", "objects/" + oid,
                                     self.packet(oid, payload=encrypted(BROWSE_PAGE_BYTES // 2)))[0], 200)
        page = json.loads(self.request("GET", "objects?limit=20")[1])
        self.assertEqual(len(page["items"]), 1)
        self.assertTrue(page["more"])

    def test_concurrent_uploads_cannot_exceed_quota(self):
        with self.database.connect() as db:
            db.execute("UPDATE vaults SET quota=100 WHERE id=?", (self.account["vault_id"],))
        barrier = threading.Barrier(2)

        def upload(_):
            barrier.wait()
            return self.request("PUT", "assets/" + uid(), encrypted(60))[0]

        with ThreadPoolExecutor(max_workers=2) as pool:
            self.assertEqual(sorted(pool.map(upload, range(2))), [200, 413])
        usage = json.loads(self.request("GET", "usage")[1])
        self.assertEqual(usage["used_bytes"], 60)
        self.assertEqual(usage["asset_count"], 1)

    def test_idempotency_cas_and_immutable_identifiers(self):
        oid, aid = uid(), uid()
        self.assertEqual(self.request("PUT", "assets/" + aid, encrypted())[0], 200)
        first = self.packet(oid, assets=[aid])
        status, response = self.request("PUT", "objects/" + oid, first)
        self.assertEqual(status, 200)
        self.assertEqual(self.request("PUT", "objects/" + oid, first), (200, response))
        usage = self.request("GET", "usage")[1]
        self.assertEqual(self.request("PUT", "assets/" + aid, encrypted())[0], 200)
        self.assertEqual(self.request("GET", "usage")[1], usage)
        self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid))[0], 409)
        mutated = json.loads(json.dumps(first))
        mutated["ciphertext"] = base64.b64encode(encrypted(61)).decode()
        self.assertEqual(self.request("PUT", "objects/" + oid, mutated)[0], 409)
        self.assertEqual(self.request("PUT", "assets/" + aid, encrypted(61))[0], 409)
        second = self.packet(oid, first["header"]["revision"], [aid], encrypted(100))
        self.assertEqual(self.request("PUT", "objects/" + oid, second)[0], 200)
        # Retrying an old accepted request cannot move the current head backwards.
        self.assertEqual(self.request("PUT", "objects/" + oid, first)[0], 200)
        changes = json.loads(self.request("GET", "changes?after=0")[1])
        self.assertEqual(changes["items"][0]["header"]["revision"], second["header"]["revision"])
        self.assertEqual(json.loads(self.request("GET", "usage")[1])["used_bytes"], 160)

    def test_missing_attachments_fail_atomically(self):
        oid = uid()
        self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid, assets=[uid()]))[0], 422)
        usage = json.loads(self.request("GET", "usage")[1])
        self.assertEqual(usage["used_bytes"], 0)
        self.assertEqual(usage["object_count"], 0)

    def test_gc_keeps_references_and_only_releases_old_orphans(self):
        used, orphan, fresh = uid(), uid(), uid()
        for aid in [used, orphan, fresh]:
            self.assertEqual(self.request("PUT", "assets/" + aid, encrypted())[0], 200)
        oid = uid()
        self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid, assets=[used]))[0], 200)
        with self.database.connect() as db:
            db.execute("UPDATE assets SET created=? WHERE id IN (?,?)", (time.time() - 90000, used, orphan))
        status, result = self.request("POST", "gc", b"{}")
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(result), {"deleted": 1, "released_bytes": 60})
        self.assertEqual(self.request("GET", "assets/" + used)[0], 200)
        self.assertEqual(self.request("GET", "assets/" + fresh)[0], 200)
        self.assertEqual(self.request("GET", "assets/" + orphan)[0], 404)
        self.assertEqual(json.loads(self.request("GET", "usage")[1])["used_bytes"], 180)

    def test_pagination_and_restart_keep_changes(self):
        for _ in range(35):
            oid = uid()
            self.assertEqual(self.request("PUT", "objects/" + oid, self.packet(oid))[0], 200)
        cursor, ids = 0, set()
        while True:
            page = json.loads(self.request("GET", f"changes?after={cursor}")[1])
            ids.update(item["header"]["object_id"] for item in page["items"])
            self.assertGreaterEqual(page["cursor"], cursor)
            cursor = page["cursor"]
            if not page["more"]:
                break
        self.assertEqual(len(ids), 35)
        restarted = Database(Path(self.temp.name) / "sync.sqlite3", 1)
        self.assertEqual(restarted.usage(self.account["vault_id"])["object_count"], 35)

    def test_removing_last_reference_starts_a_new_grace_period(self):
        aid, oid = uid(), uid()
        self.assertEqual(self.request("PUT", "assets/" + aid, encrypted())[0], 200)
        first = self.packet(oid, assets=[aid])
        self.assertEqual(self.request("PUT", "objects/" + oid, first)[0], 200)
        with self.database.connect() as db:
            db.execute("UPDATE assets SET created=? WHERE id=?", (time.time() - 90000, aid))
        second = self.packet(oid, first["header"]["revision"])
        self.assertEqual(self.request("PUT", "objects/" + oid, second)[0], 200)
        result = json.loads(self.request("POST", "gc", b"{}")[1])
        self.assertEqual(result["deleted"], 0)
        self.assertEqual(self.request("GET", "assets/" + aid)[0], 200)


if __name__ == "__main__":
    unittest.main(verbosity=2)
