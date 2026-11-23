"""Real sync API with a local Casdoor protocol fixture."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sync-server"))
from casdoor import Casdoor
from casdoor_fixture import Fixture
from server import Database, Handler, SyncHTTPServer

parser = argparse.ArgumentParser()
parser.add_argument("--data", type=Path, required=True)
parser.add_argument("--port", type=int, default=0)
parser.add_argument("--ready-file", type=Path)
args = parser.parse_args()
args.data.mkdir(parents=True, exist_ok=True)
ready_file = args.ready_file or args.data / "ready.json"
previous = json.loads(ready_file.read_text()) if ready_file.exists() else {}
issuer_port = int(previous["issuer"].split(":")[2]) if previous else 0
fixture = Fixture(port=issuer_port, state_path=args.data / "provider-tokens.json")
server = SyncHTTPServer(("127.0.0.1", args.port), Handler)
server.daemon_threads = True
server.database = Database(args.data / "sync.sqlite3", 512 * 1024 * 1024)
server.accounts = Casdoor(fixture.issuer, fixture.issuer, "suojian-test", "fixture-secret", server.database)
temporary = ready_file.with_suffix(".tmp")
temporary.write_text(json.dumps({"url": f"http://127.0.0.1:{server.server_port}", "issuer": fixture.issuer}))
temporary.replace(ready_file)
try:
    server.serve_forever()
finally:
    server.server_close()
    fixture.close()
