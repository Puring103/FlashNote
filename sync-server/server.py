"""账号同步：Casdoor 认证；新库默认客户端持钥，兼容旧托管库。

Run: python server.py --data ./data --port 47831
HTTP is allowed only for local development; use HTTPS outside localhost.
"""
import argparse
import base64
import hashlib
import json
import os
import sqlite3
import threading
import time
import uuid
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit
from identity import IdentityUnavailable
from casdoor import Casdoor

MAX_ASSET = 128 * 1024 * 1024 + 41  # file limit plus encryption overhead
MAX_OBJECT = 8 * 1024 * 1024
PAGE_SIZE = 16
BROWSE_LIMIT = 20
MAX_BROWSE_LIMIT = 50
BROWSE_PAGE_BYTES = 256 * 1024


def encode(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def valid_id(value):
    return isinstance(value, str) and str(uuid.UUID(value)) == value


class ApiError(Exception):
    def __init__(self, status, message):
        self.status, self.message = status, message


class ClosingConnection(sqlite3.Connection):
    def __exit__(self, *args):
        try:
            return super().__exit__(*args)
        finally:
            self.close()


class Database:
    def __init__(self, path, quota, e2ee=True):
        self.e2ee = e2ee
        self.path, self.quota = str(path), quota
        with self.connect() as db:
            db.executescript("""
                PRAGMA journal_mode=WAL;
                CREATE TABLE IF NOT EXISTS vaults(
                    id TEXT PRIMARY KEY, issuer TEXT NOT NULL, subject TEXT NOT NULL,
                    quota INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(issuer,subject));
                CREATE TABLE IF NOT EXISTS client_keys(
                    vault TEXT PRIMARY KEY, fingerprint TEXT);
                CREATE TABLE IF NOT EXISTS managed_keys(
                    vault TEXT PRIMARY KEY, sealed BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS receipts(
                    seq INTEGER PRIMARY KEY AUTOINCREMENT, vault TEXT NOT NULL,
                    revision TEXT NOT NULL, digest TEXT NOT NULL,
                    UNIQUE(vault,revision));
                CREATE TABLE IF NOT EXISTS objects(
                    vault TEXT NOT NULL,id TEXT NOT NULL,revision TEXT NOT NULL,
                    seq INTEGER NOT NULL,envelope BLOB NOT NULL,bytes INTEGER NOT NULL,
                    PRIMARY KEY(vault,id));
                CREATE INDEX IF NOT EXISTS objects_changes ON objects(vault,seq);
                CREATE INDEX IF NOT EXISTS objects_listing
                    ON objects(vault,json_extract(envelope,'$.header.kind'),seq);
                CREATE TABLE IF NOT EXISTS assets(
                    vault TEXT NOT NULL,id TEXT NOT NULL,payload BLOB NOT NULL,
                    digest TEXT NOT NULL,created REAL NOT NULL,
                    PRIMARY KEY(vault,id));
                CREATE TABLE IF NOT EXISTS refs(
                    vault TEXT NOT NULL,object_id TEXT NOT NULL,asset_id TEXT NOT NULL,
                    PRIMARY KEY(vault,object_id,asset_id));
            """)

    def account_info(self, identity, claim):
        if not isinstance(claim, dict) or set(claim) not in (set(), {"vault_id", "fingerprint"}):
            raise ApiError(400, "invalid account request")
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT id FROM vaults WHERE issuer=? AND subject=?",
                             (identity.issuer, identity.subject)).fetchone()
            vault = row["id"] if row else None
            if not vault and self.e2ee and not claim:
                vault = str(uuid.uuid4())
                db.execute("INSERT INTO vaults(id,issuer,subject,quota) VALUES(?,?,?,?)",
                           (vault, identity.issuer, identity.subject, self.quota))
                db.execute("INSERT INTO client_keys(vault) VALUES(?)", (vault,))
            record = db.execute("SELECT fingerprint FROM client_keys WHERE vault=?", (vault,)).fetchone()
            if claim:
                fp = claim["fingerprint"]
                if not record or claim["vault_id"] != vault or not isinstance(fp, str) or len(fp) != 64 or any(c not in "0123456789abcdef" for c in fp):
                    raise ApiError(400, "invalid key confirmation")
                db.execute("UPDATE client_keys SET fingerprint=? WHERE vault=? AND fingerprint IS NULL", (fp, vault))
                record = db.execute("SELECT fingerprint FROM client_keys WHERE vault=?", (vault,)).fetchone()
            return {"vault_id": vault, "issuer": identity.issuer, "subject": identity.subject,
                    "name": identity.name, "mode": "e2ee" if record else "managed",
                    "fingerprint": record["fingerprint"] if record else None}

    def managed_account(self, identity):
        """认证后按归属领取密钥；创建、并发登录和主密钥文件读写在同一写事务内串行。"""
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT id FROM vaults WHERE issuer=? AND subject=?",
                             (identity.issuer, identity.subject)).fetchone()
            if (not row and self.e2ee) or (row and db.execute(
                    "SELECT 1 FROM client_keys WHERE vault=?", (row["id"],)).fetchone()):
                raise ApiError(409, "end-to-end encryption requires an updated client")
            path = Path(self.path).with_name('sync-master.key')
            if not path.exists():
                if row or db.execute("SELECT 1 FROM managed_keys LIMIT 1").fetchone():
                    raise ApiError(503, "server master key unavailable")
                # 同一数据库事务防止并发首次登录生成不同主密钥；密钥文件必须与数据一起备份。
                with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'wb') as file:
                    file.write(os.urandom(32))
                    file.flush()
                    os.fsync(file.fileno())
            master = path.read_bytes()
            if len(master) != 32:
                raise ApiError(503, "server master key invalid")
            vault = row['id'] if row else str(uuid.uuid4())
            aad = encode([identity.issuer, identity.subject, vault])
            if row:
                record = db.execute("SELECT sealed FROM managed_keys WHERE vault=?", (vault,)).fetchone()
                if record is None:
                    raise ApiError(503, "account key unavailable")
                sealed = record['sealed']
                key = AESGCM(master).decrypt(sealed[:12], sealed[12:], aad)
            else:
                key, nonce = os.urandom(32), os.urandom(12)
                sealed = nonce + AESGCM(master).encrypt(nonce, key, aad)
                db.execute("INSERT INTO vaults(id,issuer,subject,quota) VALUES(?,?,?,?)",
                           (vault, identity.issuer, identity.subject, self.quota))
                db.execute("INSERT INTO managed_keys VALUES(?,?)", (vault, sealed))
        return {"issuer":identity.issuer, "subject":identity.subject, "name":identity.name,
                "vault_id":vault, "key":base64.b64encode(key).decode()}

    def connect(self):
        db = sqlite3.connect(self.path, timeout=30, factory=ClosingConnection)
        db.row_factory = sqlite3.Row
        return db

    def authenticate(self, vault, identity):
        with self.connect() as db:
            row = db.execute("""SELECT 1 FROM vaults WHERE id=? AND issuer=? AND subject=?
                AND NOT EXISTS (SELECT 1 FROM client_keys WHERE vault=vaults.id AND fingerprint IS NULL)""",
                             (vault, identity.issuer, identity.subject)).fetchone()
        if not row:
            raise ApiError(401, "unauthorized")

    @staticmethod
    def charge(db, vault, delta):
        row = db.execute("SELECT used,quota FROM vaults WHERE id=?", (vault,)).fetchone()
        if row[0] + delta > row[1]:
            raise ApiError(413, "storage quota exceeded")
        db.execute("UPDATE vaults SET used=used+? WHERE id=?", (delta, vault))

    def put_asset(self, vault, aid, payload):
        if len(payload) > MAX_ASSET:
            raise ApiError(413, "attachment too large")
        if len(payload) < 41 or not (payload[0] == 1 or (len(payload) >= 42 and payload[:2] == b"\x02\x01")):
            raise ApiError(400, "invalid encrypted asset")
        digest = hashlib.sha256(payload).hexdigest()
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT digest FROM assets WHERE vault=? AND id=?", (vault, aid)).fetchone()
            if row:
                if row[0] != digest:
                    raise ApiError(409, "asset id is immutable")
                return
            self.charge(db, vault, len(payload))
            db.execute("INSERT INTO assets VALUES(?,?,?,?,?)", (vault, aid, payload, digest, time.time()))

    def put_object(self, vault, oid, envelope):
        head = envelope["header"]
        if (set(head) != {"version", "vault_id", "object_id", "kind", "revision",
                         "base_revision", "device", "assets"}
                or head["version"] != 1 or head["vault_id"] != vault or head["object_id"] != oid
                or head["kind"] not in ("note", "group")
                or not valid_id(head["revision"]) or not valid_id(head["device"])
                or (head["base_revision"] is not None and not valid_id(head["base_revision"]))
                or not isinstance(head["assets"], list) or len(head["assets"]) > 10000
                or any(not valid_id(a) for a in head["assets"])
                or head["assets"] != sorted(set(head["assets"]))
                or (head["kind"] == "group" and head["assets"])):
            raise ApiError(400, "invalid object header")
        payload = base64.b64decode(envelope["ciphertext"], validate=True)
        if len(payload) > MAX_ASSET:
            raise ApiError(413, "attachment too large")
        if len(payload) < 41 or not (payload[0] == 1 or (len(payload) >= 42 and payload[:2] == b"\x02\x01")):
            raise ApiError(400, "invalid ciphertext")
        raw = encode(envelope)
        digest = hashlib.sha256(raw).hexdigest()
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            old = db.execute("SELECT * FROM objects WHERE vault=? AND id=?", (vault, oid)).fetchone()
            receipt = db.execute("SELECT seq,digest FROM receipts WHERE vault=? AND revision=?",
                                 (vault, head["revision"])).fetchone()
            if receipt:
                if receipt["digest"] != digest:
                    raise ApiError(409, "revision id is immutable")
                return 200, {"seq": receipt["seq"]}
            if (old["revision"] if old else None) != head["base_revision"]:
                return 409, {"current": json.loads(old["envelope"]) if old else None}
            if old and json.loads(old["envelope"])["header"]["kind"] != head["kind"]:
                raise ApiError(409, "object kind is immutable")
            for aid in head["assets"]:
                if not db.execute("SELECT 1 FROM assets WHERE vault=? AND id=?", (vault, aid)).fetchone():
                    raise ApiError(422, "referenced asset missing")
            self.charge(db, vault, len(payload) - (old["bytes"] if old else 0))
            seq = db.execute("INSERT INTO receipts(vault,revision,digest) VALUES(?,?,?)",
                             (vault, head["revision"], digest)).lastrowid
            db.execute("INSERT OR REPLACE INTO objects VALUES(?,?,?,?,?,?)",
                       (vault, oid, head["revision"], seq, raw, len(payload)))
            old_refs = {r[0] for r in db.execute("SELECT asset_id FROM refs WHERE vault=? AND object_id=?", (vault, oid))}
            db.execute("DELETE FROM refs WHERE vault=? AND object_id=?", (vault, oid))
            db.executemany("INSERT INTO refs VALUES(?,?,?)", [(vault, oid, a) for a in head["assets"]])
            for aid in old_refs - set(head["assets"]):
                db.execute("""UPDATE assets SET created=? WHERE vault=? AND id=? AND NOT EXISTS(
                    SELECT 1 FROM refs WHERE vault=? AND asset_id=?)""", (time.time(), vault, aid, vault, aid))
            return 200, {"seq": seq}

    def matches_cursor(self, vault, seq, revision):
        with self.connect() as db:
            return db.execute("SELECT 1 FROM receipts WHERE vault=? AND seq=? AND revision=?",
                              (vault, seq, revision)).fetchone() is not None

    def changes(self, vault, after):
        with self.connect() as db:
            rows = db.execute("SELECT seq,envelope FROM objects WHERE vault=? AND seq>? ORDER BY seq LIMIT ?",
                              (vault, after, PAGE_SIZE)).fetchall()
        return {"items": [json.loads(r["envelope"]) for r in rows],
                "cursor": rows[-1]["seq"] if rows else after, "more": len(rows) == PAGE_SIZE}

    def get_object(self, vault, oid):
        with self.connect() as db:
            row = db.execute("SELECT envelope FROM objects WHERE vault=? AND id=?", (vault, oid)).fetchone()
        if not row:
            raise ApiError(404, "object not found")
        return json.loads(row["envelope"])

    def list_objects(self, vault, kind=None, limit=BROWSE_LIMIT, cursor=None):
        if kind not in (None, "note", "group") or type(limit) is not int or not 1 <= limit <= MAX_BROWSE_LIMIT:
            raise ApiError(400, "invalid listing options")
        state = None
        if cursor is not None:
            try:
                if not isinstance(cursor, str) or not 1 <= len(cursor) <= 1024:
                    raise ValueError()
                state = json.loads(base64.b64decode(cursor, altchars=b"-_", validate=True))
                if (not isinstance(state, dict)
                        or set(state) != {"version", "vault", "kind", "snapshot", "before"}
                        or type(state["version"]) is not int or state["version"] != 1
                        or state["vault"] != vault or state["kind"] != kind
                        or type(state["snapshot"]) is not int or type(state["before"]) is not int
                        or not 0 <= state["before"] <= state["snapshot"] <= 2**63 - 1):
                    raise ValueError()
            except (ValueError, TypeError, UnicodeError):
                raise ApiError(400, "invalid listing cursor") from None
        with self.connect() as db:
            # Keep the watermark, candidate IDs and returned payloads in one read snapshot.
            db.execute("BEGIN")
            snapshot = db.execute("SELECT COALESCE(MAX(seq),0) FROM objects WHERE vault=?", (vault,)).fetchone()[0]
            if state and state["snapshot"] != snapshot:
                raise ApiError(409, "listing changed; restart without cursor")
            before = state["before"] if state else snapshot
            sql = "SELECT id,seq,length(envelope) AS size FROM objects WHERE vault=? AND seq<=?"
            parameters = [vault, before]
            if kind is not None:
                sql += " AND json_extract(envelope,'$.header.kind')=?"
                parameters.append(kind)
            rows = db.execute(sql + " ORDER BY seq DESC LIMIT ?", [*parameters, limit + 1]).fetchall()
            items, size = [], 0
            for row in rows[:limit]:
                if items and size + row["size"] > BROWSE_PAGE_BYTES:
                    break
                # A single large note is allowed so pagination always makes progress.
                raw = db.execute("SELECT envelope FROM objects WHERE vault=? AND id=?", (vault, row["id"])).fetchone()[0]
                items.append(json.loads(raw))
                size += row["size"]
        more = len(rows) > len(items)
        next_cursor = None
        if more:
            next_cursor = base64.urlsafe_b64encode(encode({
                "version": 1, "vault": vault, "kind": kind,
                "snapshot": snapshot, "before": rows[len(items) - 1]["seq"] - 1,
            })).decode()
        return {"items": items, "next_cursor": next_cursor, "more": more, "snapshot": snapshot}

    def usage(self, vault):
        with self.connect() as db:
            row = db.execute("SELECT used,quota FROM vaults WHERE id=?", (vault,)).fetchone()
            assets = db.execute("SELECT COALESCE(SUM(length(payload)),0),COUNT(*) FROM assets WHERE vault=?", (vault,)).fetchone()
            objects = db.execute("SELECT COALESCE(SUM(bytes),0),COUNT(*) FROM objects WHERE vault=?", (vault,)).fetchone()
        return {"used_bytes": row[0], "quota_bytes": row[1], "asset_bytes": assets[0],
                "asset_count": assets[1], "object_bytes": objects[0], "object_count": objects[1]}

    def gc(self, vault):
        # No history expiry: only unreferenced attachments older than one day.
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            rows = db.execute("""SELECT id,length(payload) AS bytes FROM assets a
                WHERE vault=? AND created<? AND NOT EXISTS(
                SELECT 1 FROM refs r WHERE r.vault=a.vault AND r.asset_id=a.id)""",
                              (vault, time.time() - 86400)).fetchall()
            for row in rows:
                db.execute("DELETE FROM assets WHERE vault=? AND id=?", (vault, row["id"]))
            released = sum(r["bytes"] for r in rows)
            self.charge(db, vault, -released)
        return {"deleted": len(rows), "released_bytes": released}


class Handler(BaseHTTPRequestHandler):
    server_version = "SuojianSync/1"

    def log_message(self, *_):
        pass  # Never log authorization, payloads, object names or request bodies.

    def body(self, limit):
        length = int(self.headers.get("Content-Length", "-1"))
        if length < 0:
            raise ApiError(411, "content length required")
        if length > limit:
            raise ApiError(413, "request too large")
        result = self.rfile.read(length)
        if len(result) != length:
            raise ApiError(400, "incomplete body")
        return result

    def reply(self, status, value, binary=False):
        payload = value if binary else encode(value)
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream" if binary else "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(payload)

    def dispatch(self):
        self.connection.settimeout(30)
        db = self.server.database
        parts = urlsplit(self.path)
        path = parts.path.strip("/").split("/")
        accounts = self.server.accounts
        if self.command == "GET" and path == ["health"]:
            return self.reply(200, {"ok": True, "version": "0.12.0", "protocol": 3, "authentication": "casdoor"})
        if self.command == "GET" and path == ["auth", "config"]:
            return self.reply(200, {**accounts.configuration(), "account_login": True, "vault_protocol": 1,
                                    "new_vault_encryption": "e2ee" if db.e2ee else "managed",
                                    "registration_enabled": getattr(self.server, "registration_enabled", False), "mode": "casdoor"})
        if self.command == "POST" and path in (["auth", "login"], ["auth", "refresh"], ["auth", "logout"]):
            body = json.loads(self.body(4096))
            try:
                if path[-1] == "login":
                    value = accounts.login(body["username"], body["secret"], self.client_address[0])
                elif path[-1] == "refresh":
                    value = accounts.refresh(body["refresh_token"])
                else:
                    accounts.revoke(body["refresh_token"])
                    value = {"ok": True}
            except IdentityUnavailable:
                raise ApiError(503, "identity service unavailable") from None
            except (ValueError, KeyError, TypeError):
                raise ApiError(401, "invalid credentials or expired session") from None
            return self.reply(200, value)
        try:
            authorization = self.headers.get("Authorization", "")
            identity = accounts.verify(authorization)
        except IdentityUnavailable:
            raise ApiError(503, "identity service unavailable") from None
        except Exception:
            raise ApiError(401, "valid login required") from None
        if self.command == "POST" and path == ["v1", "account", "vault"]:
            return self.reply(200, db.account_info(identity, json.loads(self.body(1024))))
        if self.command == "POST" and path == ["v1", "account", "key"]:
            self.body(1024)
            return self.reply(200, db.managed_account(identity))
        if len(path) < 3 or path[0] != "v1" or not valid_id(path[1]):
            raise ApiError(404, "not found")
        vault, route = path[1], path[2]
        db.authenticate(vault, identity)
        if len(path) == 3:
            if self.command == "GET" and route == "objects":
                query = parse_qs(parts.query, keep_blank_values=True)
                if set(query) - {"kind", "limit", "cursor"} or any(len(values) != 1 for values in query.values()):
                    raise ApiError(400, "invalid listing query")
                return self.reply(200, db.list_objects(vault, query.get("kind", [None])[0],
                    int(query.get("limit", [str(BROWSE_LIMIT)])[0]), query.get("cursor", [None])[0]))
            if self.command == "GET" and route == "checkpoint":
                query = parse_qs(parts.query)
                return self.reply(200, {"valid": db.matches_cursor(vault, int(query["seq"][0]), query["revision"][0])})
            if self.command == "GET" and route == "changes":
                after = int(parse_qs(parts.query).get("after", ["0"])[0])
                if after < 0:
                    raise ApiError(400, "invalid cursor")
                return self.reply(200, db.changes(vault, after))
            if self.command == "GET" and route == "usage":
                return self.reply(200, db.usage(vault))
            if self.command == "POST" and route == "gc":
                self.body(1024)
                return self.reply(200, db.gc(vault))
        if len(path) == 4 and valid_id(path[3]):
            oid = path[3]
            if route == "assets":
                if self.command == "PUT":
                    db.put_asset(vault, oid, self.body(MAX_ASSET))
                    return self.reply(200, {"ok": True})
                if self.command in ("GET", "HEAD"):
                    with db.connect() as conn:
                        field = "length(payload)" if self.command == "HEAD" else "payload"
                        row = conn.execute(f"SELECT {field} FROM assets WHERE vault=? AND id=?", (vault, oid)).fetchone()
                    if not row:
                        raise ApiError(404, "asset not found")
                    if self.command == "HEAD":
                        self.send_response(200)
                        self.send_header("Content-Type", "application/octet-stream")
                        self.send_header("Content-Length", str(row[0]))
                        self.send_header("Cache-Control", "no-store")
                        self.end_headers()
                        return
                    return self.reply(200, row[0], binary=True)
            if route == "objects":
                if self.command == "PUT":
                    status, result = db.put_object(vault, oid, json.loads(self.body(MAX_OBJECT)))
                    return self.reply(status, result)
                if self.command in ("GET", "HEAD"):
                    return self.reply(200, db.get_object(vault, oid))
        raise ApiError(404, "not found")

    def handle_request(self):
        try:
            self.dispatch()
        except ApiError as error:
            self.reply(error.status, {"error": error.message})
        except (ValueError, KeyError, TypeError, AttributeError):
            self.reply(400, {"error": "invalid request"})
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass
        except Exception:
            self.reply(500, {"error": "storage operation failed"})

    do_GET = do_HEAD = do_POST = do_PUT = handle_request


class SyncHTTPServer(ThreadingHTTPServer):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.workers = threading.BoundedSemaphore(16)

    def process_request(self, request, client_address):
        request.settimeout(30)
        if not self.workers.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.workers.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.workers.release()

    def service_actions(self):
        if time.monotonic() < getattr(self, "next_gc", 0):
            return
        self.next_gc = time.monotonic() + 3600
        try:
            with self.database.connect() as db:
                vaults = [r[0] for r in db.execute("SELECT id FROM vaults")]
            for vault in vaults:
                self.database.gc(vault)
        except sqlite3.Error:
            pass  # Retry next hour; a cleanup failure must not discard user data.


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--data", type=Path, default=Path("data"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=47831)
    parser.add_argument("--quota", type=int, default=512 * 1024 * 1024)
    parser.add_argument("--ready-file", type=Path)
    args = parser.parse_args()
    if args.quota <= 0:
        parser.error("quota must be positive")
    args.data.mkdir(parents=True, exist_ok=True)
    policy = os.environ.get("SUOJIAN_E2EE", "true").lower()
    if policy not in ("true", "false"):
        parser.error("SUOJIAN_E2EE must be true or false")
    database = Database(args.data / "sync.sqlite3", args.quota, e2ee=policy == "true")
    client_secret = os.environ.get("CASDOOR_CLIENT_SECRET", "")
    if secret_file := os.environ.get("CASDOOR_CLIENT_SECRET_FILE"):
        client_secret = Path(secret_file).read_text(encoding="utf-8").rstrip("\r\n")
    accounts = Casdoor(os.environ.get("CASDOOR_ENDPOINT", "http://127.0.0.1:19000"),
                      os.environ.get("CASDOOR_ISSUER", ""),
                      os.environ.get("CASDOOR_CLIENT_ID", ""),
                      client_secret, database)
    server = SyncHTTPServer((args.host, args.port), Handler)
    registration = os.environ.get("SUOJIAN_REGISTRATION", "false").lower()
    if registration not in ("true", "false"):
        parser.error("SUOJIAN_REGISTRATION must be true or false")
    server.registration_enabled = registration == "true"
    server.daemon_threads = True
    server.database = database
    server.accounts = accounts
    address = f"http://127.0.0.1:{server.server_port}"
    if args.ready_file:
        temporary = args.ready_file.with_suffix(".tmp")
        temporary.write_text(json.dumps({"url": address}), encoding="utf-8")
        temporary.replace(args.ready_file)
    print(f"Suojian sync listening on {address}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
