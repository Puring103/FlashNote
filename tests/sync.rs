//! These tests start real HTTP servers and isolated vaults.
#[path = "support/test_store.rs"]
mod test_store;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use suojian::{auth, model::*, store::Store, sync};
use tempfile::TempDir;

fn client_db(store: &Store) -> rusqlite::Connection {
    rusqlite::Connection::open(store.root.join("notes.sqlite3")).unwrap()
}

struct Server {
    child: Child,
    temp: TempDir,
    url: String,
}
impl Server {
    fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready.json");
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/serve_sync.py");
        let mut command =
            Command::new(std::env::var("SUOJIAN_TEST_PYTHON").unwrap_or_else(|_| "python".into()));
        command
            .arg(script)
            .args(["--port", "0", "--data"])
            .arg(temp.path())
            .arg("--ready-file")
            .arg(&ready)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command
            .spawn()
            .expect("Python 3 is required for the HTTP integration tests");
        let mut server = Self {
            child,
            temp,
            url: String::new(),
        };
        let started = Instant::now();
        loop {
            if let Ok(bytes) = std::fs::read(&ready) {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                server.url = value["url"].as_str().unwrap().into();
                return server;
            }
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "sync server exited"
            );
            if started.elapsed() > Duration::from_secs(10) {
                panic!("server startup timed out");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.temp.path().join("sync.sqlite3")).unwrap()
    }
    fn session(&self) -> auth::Session {
        self.session_as("alice")
    }
    fn session_as(&self, user: &str) -> auth::Session {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        auth::account_login(&client, &self.url, user, "login123").unwrap()
    }

    fn connect(&self, store: &mut Store) {
        let connection = sync::account_with_session(&self.url, self.session()).unwrap();
        sync::connect_account(store, connection).unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pair(server: &Server) -> (TempDir, Store, Store) {
    let temp = tempfile::tempdir().unwrap();
    let mut a = test_store::open_store(temp.path().join("a")).unwrap();
    server.connect(&mut a);
    sync::once(&mut a).unwrap();
    let mut b = test_store::open_store(temp.path().join("b")).unwrap();
    server.connect(&mut b);
    sync::once(&mut b).unwrap();
    assert_ne!(a.device, b.device);
    (temp, a, b)
}

#[test]
fn server_address_is_required_and_defaults_to_https() {
    for (input, expected) in [
        (" sync.example.com ", "https://sync.example.com"),
        ("203.0.113.10:8443", "https://203.0.113.10:8443"),
        ("2001:db8::1", "https://[2001:db8::1]"),
        ("https://sync.example.com/", "https://sync.example.com"),
        ("http://127.0.0.1:47831", "http://127.0.0.1:47831"),
    ] {
        assert_eq!(sync::parse_server_address(input).unwrap(), expected);
    }
    for input in [
        "",
        "http://203.0.113.10",
        "https://sync.example.com/login",
        "https://user@sync.example.com",
        "https://sync.example.com?query",
    ] {
        assert!(sync::parse_server_address(input).is_err(), "{input}");
    }
}

#[test]
fn legacy_session_requires_explicit_server_address_without_unbinding_vault() {
    let server = Server::start();
    let temp = tempfile::tempdir().unwrap();
    let mut store = test_store::open_store(temp.path().join("vault")).unwrap();
    server.connect(&mut store);
    let note_id = add(&mut store, "local note");
    let mut config = sync::config(&store).unwrap().unwrap();
    config.version = 2;
    let session = suojian::crypto::protect(&serde_json::to_vec(&config).unwrap(), false).unwrap();
    std::fs::write(store.root.join("sync-session.dpapi"), session).unwrap();
    let owner = store.root.join("sync-owner.json");
    let binding = std::fs::read(&owner).unwrap();

    assert!(sync::config(&store).unwrap().is_none());
    assert!(sync::once(&mut store).is_err());
    let connection = sync::login(&server.url, "alice", "login123", Default::default()).unwrap();
    sync::connect_account(&mut store, connection).unwrap();
    assert_eq!(std::fs::read(owner).unwrap(), binding);
    assert_eq!(store.notes[0].id, note_id);
    assert!(sync::config(&store).unwrap().is_some());
}

fn add(store: &mut Store, text: &str) -> String {
    let mut n = Note::new();
    n.set_markdown(text.into());
    let nid = n.id.clone();
    store.save_note(n).unwrap();
    nid
}
fn edit(store: &mut Store, nid: &str, text: &str) {
    let mut n = store.notes.iter().find(|n| n.id == nid).unwrap().clone();
    n.set_markdown(text.into());
    store.save_note(n).unwrap();
}
fn text(store: &Store, nid: &str) -> String {
    store.notes.iter().find(|n| n.id == nid).unwrap().text()
}
fn active(store: &Store) -> Vec<&Note> {
    store
        .notes
        .iter()
        .filter(|n| n.deleted_at.is_none() && !n.is_empty())
        .collect()
}
fn converge(a: &mut Store, b: &mut Store) {
    for _ in 0..3 {
        sync::once(a).unwrap();
        sync::once(b).unwrap();
    }
}

#[test]
fn group_migration_and_permanent_deletion_converge() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let group = a.create_group_with_color("整理测试", 0x245356).unwrap();
    let nid = add(&mut a, "需要迁移的便签");
    a.batch_notes(&[nid.clone()], Some(&group.id), None)
        .unwrap();
    converge(&mut a, &mut b);
    assert!(b.groups.iter().any(|g| g.id == group.id));
    a.delete_group(&group.id, DEFAULT_GROUP).unwrap();
    converge(&mut a, &mut b);
    assert!(!b.groups.iter().any(|g| g.id == group.id));
    assert_eq!(
        b.notes.iter().find(|n| n.id == nid).unwrap().group_id,
        DEFAULT_GROUP
    );
    a.batch_notes(&[nid.clone()], None, Some(true)).unwrap();
    converge(&mut a, &mut b);
    a.purge_notes(&[nid.clone()]).unwrap();
    converge(&mut a, &mut b);
    let deleted = b.notes.iter().find(|n| n.id == nid).unwrap();
    assert!(deleted.is_empty() && deleted.deleted_at.is_some());
}

#[test]
fn all_conflict_choices_sync_resolution_and_reject_stale_comparisons() {
    use suojian::store::ConflictChoice;
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    for choice in [
        ConflictChoice::Local,
        ConflictChoice::Cloud,
        ConflictChoice::Both,
    ] {
        let nid = add(&mut a, "共同起点");
        converge(&mut a, &mut b);
        edit(&mut a, &nid, "云端版本");
        edit(&mut b, &nid, "本地版本");
        converge(&mut a, &mut b);
        let (fork, cloud) = b.conflict_pair(&nid).unwrap();
        assert!(
            b.resolve_conflict(&fork.id, "old revision", &cloud.revision, choice)
                .is_err()
        );
        // Inject a failure after the canonical note is written: no partial decision.
        client_db(&b).execute_batch("CREATE TRIGGER fail_decision BEFORE INSERT ON outbox BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        assert!(
            b.resolve_conflict(&fork.id, &fork.revision, &cloud.revision, choice)
                .is_err()
        );
        assert_eq!(b.conflict_pair(&nid).unwrap().0.revision, fork.revision);
        client_db(&b)
            .execute_batch("DROP TRIGGER fail_decision;")
            .unwrap();
        b.resolve_conflict(&fork.id, &fork.revision, &cloud.revision, choice)
            .unwrap();
        converge(&mut a, &mut b);
        for store in [&a, &b] {
            assert!(store.conflict_pair(&nid).is_none());
            let resolved = store.notes.iter().find(|n| n.id == fork.id).unwrap();
            assert!(resolved.conflict_resolved);
            assert_eq!(
                resolved.deleted_at.is_none(),
                matches!(choice, ConflictChoice::Both)
            );
            assert_eq!(
                text(store, &nid),
                if matches!(choice, ConflictChoice::Local) {
                    "本地版本"
                } else {
                    "云端版本"
                }
            );
            let versions: i64 = client_db(store)
                .query_row(
                    "SELECT COUNT(*) FROM revisions WHERE object_id=?",
                    [&fork.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(versions >= 2);
        }
        let root = b.root.clone();
        drop(b);
        b = test_store::open_store(root).unwrap();
        assert!(b.conflict_pair(&nid).is_none());
    }
}

#[test]
fn remote_image_http_failure_restart_download_and_encrypted_sync() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{address}/image-private-name");
    let mut note = Note::new();
    note.set_editor_markdown(&format!("粘贴前 ![秘密图片]({url}) 粘贴后"));
    let nid = note.id.clone();
    let bid = note
        .blocks
        .iter()
        .find(|b| matches!(b, Block::RemoteImage { .. }))
        .unwrap()
        .id()
        .to_string();
    a.save_note(note).unwrap();
    assert!(suojian::remote_images::download(&url).is_err());
    let root = a.root.clone();
    drop(a);
    let mut a = test_store::open_store(root).unwrap();
    // Pending source survives restart and sync, and remains encrypted in cloud.
    converge(&mut a, &mut b);
    assert!(
        b.notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .blocks
            .iter()
            .any(|b| matches!(b, Block::RemoteImage { .. }))
    );
    let listener = TcpListener::bind(address).unwrap();
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 8192];
        let len = socket.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..len]).to_lowercase();
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("cookie:"));
        let bytes = include_bytes!("../assets/demo/desk.jpg");
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: image/jpeg\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
        socket.write_all(bytes).unwrap();
    });
    let prepared = suojian::remote_images::download(&url).unwrap();
    worker.join().unwrap();
    let independent =
        suojian::media::prepare_bytes(prepared.name.clone(), prepared.bytes.clone(), true).unwrap();
    a.complete_remote_image(&nid, &bid, &url, prepared).unwrap();
    b.complete_remote_image(&nid, &bid, &url, independent)
        .unwrap();
    converge(&mut a, &mut b);
    assert!(a.conflict_pair(&nid).is_none());
    assert!(b.conflict_pair(&nid).is_none());
    let aid = b
        .notes
        .iter()
        .find(|n| n.id == nid)
        .unwrap()
        .blocks
        .iter()
        .find_map(|b| {
            if let Block::Image { asset, .. } = b {
                Some(asset)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        b.asset(aid).unwrap(),
        include_bytes!("../assets/demo/desk.jpg")
    );
    assert!(text(&b, &nid).contains("粘贴后"));
    let db = server.db();
    let bytes: Vec<Vec<u8>> = db
        .prepare("SELECT envelope FROM objects UNION ALL SELECT payload FROM assets")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for bytes in bytes {
        let data = String::from_utf8_lossy(&bytes);
        assert!(!data.contains("image-private-name"));
        assert!(!data.contains("粘贴前"));
    }
}

#[test]
fn offline_devices_union_notes_groups_and_encrypted_attachments() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let ga = a.create_group_with_color("电脑分组", 0x123456).unwrap();
    let gb = b.create_group_with_color("手机分组", 0x654321).unwrap();
    let aid = a.import_bytes(b"private attachment A").unwrap();
    let bid = b.import_bytes(b"private attachment B").unwrap();
    let an = add(&mut a, "电脑离线文字");
    let bn = add(&mut b, "手机离线文字");
    for (store, nid, asset, group) in [(&mut a, &an, &aid, &ga.id), (&mut b, &bn, &bid, &gb.id)] {
        let mut n = store.notes.iter().find(|n| &n.id == nid).unwrap().clone();
        n.group_id = group.clone();
        n.blocks.push(Block::File {
            id: id(),
            asset: asset.clone(),
            name: "保密文件.txt".into(),
        });
        store.save_note(n).unwrap();
    }
    b.save_settings(Settings {
        model_path: "C:/only-on-phone/secret-model".into(),
        ..Settings::default()
    })
    .unwrap();
    converge(&mut a, &mut b);
    assert_eq!(active(&a).len(), 2);
    assert_eq!(active(&b).len(), 2);
    assert_eq!(a.groups.len(), 3);
    assert_eq!(b.groups.len(), 3);
    assert_eq!(a.groups, b.groups);
    assert_eq!(a.groups[0].id, DEFAULT_GROUP);
    assert_eq!(a.asset(&bid).unwrap(), b"private attachment B");
    assert_eq!(b.asset(&aid).unwrap(), b"private attachment A");
    assert_ne!(a.settings.model_path, b.settings.model_path);
    let usage = sync::usage(&a).unwrap();
    assert_eq!(usage.asset_count, 2);
    assert_eq!(usage.object_count, 5);
    assert_eq!(usage.used_bytes, usage.asset_bytes + usage.object_bytes);
    let before = usage.clone();
    converge(&mut a, &mut b);
    assert_eq!(before, sync::usage(&b).unwrap());
    let db = server.db();
    let bytes: Vec<Vec<u8>> = db
        .prepare("SELECT envelope FROM objects UNION ALL SELECT payload FROM assets")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for bytes in bytes {
        let view = String::from_utf8_lossy(&bytes);
        for private in [
            "电脑离线文字",
            "手机离线文字",
            "保密文件",
            "private attachment",
            "secret-model",
        ] {
            assert!(!view.contains(private));
        }
        assert!(!bytes.windows(32).any(|w| w == &a.key.0[..]));
    }
}

#[test]
fn different_lines_of_the_same_note_merge_without_a_copy() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "第一行\n\n中间内容\n\n最后一行\n");
    converge(&mut a, &mut b);
    edit(&mut a, &nid, "电脑修改第一行\n\n中间内容\n\n最后一行\n");
    edit(&mut b, &nid, "第一行\n\n中间内容\n\n手机修改最后一行\n");
    sync::once(&mut a).unwrap();
    let report = sync::once(&mut b).unwrap();
    assert_eq!(report.merged, 1);
    assert_eq!(report.conflicts, 0);
    converge(&mut a, &mut b);
    assert_eq!(text(&a, &nid), text(&b, &nid));
    assert!(text(&a, &nid).contains("电脑修改第一行"));
    assert!(text(&a, &nid).contains("手机修改最后一行"));
    assert_eq!(active(&a).len(), 1);
}

#[test]
fn overlapping_edits_keep_both_and_retries_do_not_duplicate_conflict_copies() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "共同起点");
    converge(&mut a, &mut b);
    edit(&mut a, &nid, "电脑离线版本");
    edit(&mut b, &nid, "手机离线版本");
    sync::once(&mut a).unwrap();
    assert_eq!(sync::once(&mut b).unwrap().conflicts, 1);
    converge(&mut a, &mut b);
    assert_eq!(active(&a).len(), 2);
    assert_eq!(active(&b).len(), 2);
    assert!(active(&a).iter().any(|n| n.text() == "电脑离线版本"));
    assert!(
        active(&a)
            .iter()
            .any(|n| n.text() == "手机离线版本" && n.conflict_of.as_deref() == Some(nid.as_str()))
    );
    converge(&mut a, &mut b);
    assert_eq!(active(&a).len(), 2);
}

#[test]
fn deletion_vs_offline_edit_preserves_edit_and_trash_state() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "原始内容");
    converge(&mut a, &mut b);
    let mut deleted = a.notes.iter().find(|n| n.id == nid).unwrap().clone();
    deleted.deleted_at = Some(now());
    a.save_note(deleted).unwrap();
    edit(&mut b, &nid, "不能丢失的离线修改");
    sync::once(&mut a).unwrap();
    assert_eq!(sync::once(&mut b).unwrap().conflicts, 1);
    converge(&mut a, &mut b);
    assert!(
        a.notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .deleted_at
            .is_some()
    );
    assert!(active(&a).iter().any(|n| n.text() == "不能丢失的离线修改"));
    let mut restored = a.notes.iter().find(|n| n.id == nid).unwrap().clone();
    restored.deleted_at = None;
    a.save_note(restored).unwrap();
    converge(&mut a, &mut b);
    assert!(
        b.notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .deleted_at
            .is_none()
    );
}

#[test]
fn lost_ack_retries_identical_ciphertext_without_double_charging() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "已经上传但确认丢失");
    let cycle = sync::transfer(sync::prepare(&mut a).unwrap().unwrap()).unwrap();
    drop(cycle); // Simulate the process dying before the local acknowledgement.
    let usage = sync::usage(&a).unwrap();
    let report = sync::once(&mut a).unwrap();
    assert_eq!(report.uploaded, 1);
    assert_eq!(usage, sync::usage(&a).unwrap());
    converge(&mut a, &mut b);
    assert_eq!(text(&b, &nid), "已经上传但确认丢失");
}

#[test]
fn immutable_revision_error_does_not_look_like_success_or_acknowledge_edits() {
    let server = Server::start();
    let (_temp, mut a, _b) = pair(&server);
    let nid = add(&mut a, "失败时仍保留的本地修改");
    drop(sync::transfer(sync::prepare(&mut a).unwrap().unwrap()).unwrap());
    // Simulate an immutable-revision violation on a lost-ack retry.
    let revision = a
        .notes
        .iter()
        .find(|n| n.id == nid)
        .unwrap()
        .revision
        .clone();
    let cursor: String = client_db(&a)
        .query_row("SELECT value FROM sync_state WHERE key='cursor'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let db = server.db();
    db.execute(
        "UPDATE receipts SET digest='injected-mismatch' WHERE revision=?",
        [&revision],
    )
    .unwrap();
    let error = sync::once(&mut a).unwrap_err();
    assert!(error.to_string().contains("HTTP 409"));
    assert_eq!(
        cursor,
        client_db(&a)
            .query_row::<String, _, _>("SELECT value FROM sync_state WHERE key='cursor'", [], |r| r
                .get(0))
            .unwrap()
    );
    assert_eq!(
        0,
        client_db(&a)
            .query_row::<i64, _, _>(
                "SELECT acknowledged FROM outbox WHERE revision=?",
                [&revision],
                |r| r.get(0)
            )
            .unwrap()
    );
    assert_eq!(text(&a, &nid), "失败时仍保留的本地修改");
}

#[test]
fn edits_during_network_transfer_stay_pending_and_are_uploaded_next() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "请求发出时的文字");
    let plan = sync::prepare(&mut a).unwrap().unwrap();
    edit(&mut a, &nid, "请求期间继续输入的文字");
    sync::apply(&mut a, sync::transfer(plan).unwrap()).unwrap();
    assert_eq!(text(&a, &nid), "请求期间继续输入的文字");
    converge(&mut a, &mut b);
    assert_eq!(text(&b, &nid), "请求期间继续输入的文字");
}

#[test]
fn pull_commits_pages_and_resumes_after_a_later_failure() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    for i in 0..20 {
        add(&mut a, &format!("分页便签 {i}"));
    }
    assert_eq!(sync::once(&mut a).unwrap().uploaded, 20);

    let first = sync::transfer(sync::prepare_pull(&mut b).unwrap().unwrap()).unwrap();
    let stale = sync::transfer(sync::prepare_pull(&mut b).unwrap().unwrap()).unwrap();
    let report = sync::apply(&mut b, first).unwrap();
    assert!(report.more);
    assert!(report.downloaded > 0 && report.downloaded < 20);
    assert!(sync::apply(&mut b, stale).is_err());
    let received = report.downloaded;

    let db = client_db(&b);
    db.execute_batch("CREATE TRIGGER block_page BEFORE INSERT ON sync_heads BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
    assert!(sync::once(&mut b).is_err());
    assert_eq!(active(&b).len(), received);
    db.execute_batch("DROP TRIGGER block_page;").unwrap();

    let root = b.root.clone();
    drop(b);
    let mut b = test_store::open_store(root).unwrap();
    let report = sync::once(&mut b).unwrap();
    assert_eq!(report.downloaded, 20 - received);
    assert_eq!(active(&b).len(), 20);
}

#[test]
fn tampered_header_is_rejected_without_advancing_cursor() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let cursor: String = client_db(&b)
        .query_row("SELECT value FROM sync_state WHERE key='cursor'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let nid = add(&mut a, "认证保护的内容");
    sync::once(&mut a).unwrap();
    let db = server.db();
    let original: Vec<u8> = db
        .query_row("SELECT envelope FROM objects WHERE id=?", [&nid], |r| {
            r.get(0)
        })
        .unwrap();
    let mut packet: serde_json::Value = serde_json::from_slice(&original).unwrap();
    packet["header"]["device"] = id().into();
    db.execute(
        "UPDATE objects SET envelope=? WHERE id=?",
        rusqlite::params![serde_json::to_vec(&packet).unwrap(), nid],
    )
    .unwrap();
    assert!(sync::once(&mut b).is_err());
    assert_eq!(
        cursor,
        client_db(&b)
            .query_row::<String, _, _>("SELECT value FROM sync_state WHERE key='cursor'", [], |r| r
                .get(0))
            .unwrap()
    );
    assert!(b.notes.iter().all(|n| n.id != nid));
    db.execute(
        "UPDATE objects SET envelope=? WHERE id=?",
        rusqlite::params![original, nid],
    )
    .unwrap();
    sync::once(&mut b).unwrap();
    assert_eq!(text(&b, &nid), "认证保护的内容");
}

#[test]
fn failed_local_transaction_rolls_back_objects_heads_and_cursor() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "事务必须完整提交");
    sync::once(&mut a).unwrap();
    let cursor: String = client_db(&b)
        .query_row("SELECT value FROM sync_state WHERE key='cursor'", [], |r| {
            r.get(0)
        })
        .unwrap();
    client_db(&b).execute_batch("CREATE TRIGGER fail_sync BEFORE INSERT ON sync_heads BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
    assert!(sync::once(&mut b).is_err());
    assert_eq!(
        cursor,
        client_db(&b)
            .query_row::<String, _, _>("SELECT value FROM sync_state WHERE key='cursor'", [], |r| r
                .get(0))
            .unwrap()
    );
    assert_eq!(
        0,
        client_db(&b)
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM objects WHERE id=?", [&nid], |r| r
                .get(0))
            .unwrap()
    );
    client_db(&b)
        .execute_batch("DROP TRIGGER fail_sync")
        .unwrap();
    sync::once(&mut b).unwrap();
    assert_eq!(text(&b, &nid), "事务必须完整提交");
}

#[test]
fn restart_keeps_sync_cursor_and_pending_offline_changes() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "重启前的离线文字");
    let root = a.root.clone();
    drop(a);
    let mut a = test_store::open_store(root).unwrap();
    converge(&mut a, &mut b);
    assert_eq!(text(&b, &nid), "重启前的离线文字");
    assert_eq!(sync::once(&mut a).unwrap().uploaded, 0);
}

#[test]
fn account_key_requires_verified_login() {
    let server = Server::start();
    let (temp, a, _b) = pair(&server);
    let mut third = test_store::open_store(temp.path().join("third")).unwrap();
    let key_before = *third.key.0;
    // 伪造访问令牌不能取得账号密钥，失败不修改本机密钥和登录状态。
    let mut forged = server.session();
    forged.token = "invalid-access-token".into();
    assert!(sync::account_with_session(&server.url, forged).is_err());
    assert!(sync::config(&third).unwrap().is_none());
    assert_eq!(*third.key.0, key_before);
    server.connect(&mut third);
    assert_eq!(*third.key.0, *a.key.0);
    assert_ne!(
        sync::config(&third).unwrap().unwrap().session.refresh_token,
        sync::config(&a).unwrap().unwrap().session.refresh_token
    );
}

#[test]
fn token_rotation_is_saved_when_later_upload_fails_and_logout_revokes_refresh() {
    let server = Server::start();
    let temp = tempfile::tempdir().unwrap();
    let mut store = test_store::open_store(temp.path().join("vault")).unwrap();
    let mut connection = sync::account_with_session(&server.url, server.session()).unwrap();
    let original_refresh = connection.config.session.refresh_token.clone();
    connection.config.session.expires_at = 0;
    sync::connect_account(&mut store, connection).unwrap();
    add(
        &mut store,
        "必须在刷新成功、上传失败后仍保留离线编辑与新令牌",
    );
    server
        .db()
        .execute("UPDATE vaults SET quota=1", [])
        .unwrap();
    assert!(sync::once(&mut store).is_err());
    let refreshed = sync::config(&store).unwrap().unwrap();
    assert_ne!(refreshed.session.refresh_token, original_refresh);
    assert!(refreshed.session.expires_at > auth::now() + 60);
    server
        .db()
        .execute("UPDATE vaults SET quota=1048576", [])
        .unwrap();
    sync::once(&mut store).unwrap();
    let session = sync::logout(&store).unwrap().unwrap();
    assert!(sync::config(&store).unwrap().is_none());
    sync::revoke_session(&session).unwrap();
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    assert!(auth::refresh(&client, &session.session).is_err());
    assert!(!store.notes.is_empty());
}

#[test]
fn logout_rejects_inflight_sync_and_relogin_keeps_owner_and_local_notes() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "退出时仍保留本机修改");
    let cycle = sync::transfer(sync::prepare(&mut a).unwrap().unwrap()).unwrap();
    let key = *a.key.0;
    sync::logout(&a).unwrap();
    assert!(sync::apply(&mut a, cycle).is_err());
    assert!(sync::prepare(&mut a).unwrap().is_none());
    assert_eq!(text(&a, &nid), "退出时仍保留本机修改");
    server.connect(&mut a);
    assert_eq!(*a.key.0, key);
    converge(&mut a, &mut b);
    assert_eq!(text(&b, &nid), "退出时仍保留本机修改");
}

#[test]
fn switching_accounts_cannot_upload_another_accounts_local_notes() {
    let server = Server::start();
    let (_temp, mut a, _b) = pair(&server);
    let nid = add(&mut a, "账号 A 的私有便签");
    let previous = sync::config(&a).unwrap().unwrap();
    let bob = sync::account_with_session(&server.url, server.session_as("bob")).unwrap();
    assert!(sync::connect_account(&mut a, bob).is_err());
    assert_eq!(sync::config(&a).unwrap().unwrap().subject, previous.subject);
    assert_eq!(text(&a, &nid), "账号 A 的私有便签");
}

#[test]
fn network_outage_keeps_pending_changes_and_recovers_after_server_restart() {
    let mut server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    server.child.kill().unwrap();
    server.child.wait().unwrap();
    let nid = add(&mut a, "断网期间的真实修改");
    assert!(sync::once(&mut a).is_err());
    let pending: i64 = client_db(&a)
        .query_row(
            "SELECT COUNT(*) FROM outbox WHERE acknowledged=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pending > 0);
    let port = reqwest::Url::parse(&server.url).unwrap().port().unwrap();
    let mut command = Command::new("python");
    command
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/serve_sync.py"))
        .args(["--port", &port.to_string(), "--data"])
        .arg(server.temp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    server.child = command.spawn().unwrap();
    let c = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let start = Instant::now();
    while c.get(format!("{}/health", server.url)).send().is_err() {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(20));
    }
    converge(&mut a, &mut b);
    assert_eq!(text(&b, &nid), "断网期间的真实修改");
}

#[test]
fn independent_group_fields_and_note_metadata_merge() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let group = a.create_group_with_color("原分组", 0x123456).unwrap();
    let nid = add(&mut a, "原正文");
    converge(&mut a, &mut b);
    let mut ag = a.groups.iter().find(|g| g.id == group.id).unwrap().clone();
    ag.name = "电脑改名".into();
    a.save_group(ag).unwrap();
    let mut bg = b.groups.iter().find(|g| g.id == group.id).unwrap().clone();
    bg.color = 0xabcdef;
    b.save_group(bg).unwrap();
    let mut n = a.notes.iter().find(|n| n.id == nid).unwrap().clone();
    n.group_id = group.id.clone();
    a.save_note(n).unwrap();
    edit(&mut b, &nid, "手机改正文");
    converge(&mut a, &mut b);
    for store in [&a, &b] {
        let g = store.groups.iter().find(|g| g.id == group.id).unwrap();
        assert_eq!(g.name, "电脑改名");
        assert_eq!(g.color, 0xabcdef);
        let n = store.notes.iter().find(|n| n.id == nid).unwrap();
        assert_eq!(n.group_id, group.id);
        assert_eq!(n.text(), "手机改正文");
        assert!(n.conflict_of.is_none());
    }
}

#[test]
fn conflict_copy_retains_attachment_bytes_on_both_devices() {
    let server = Server::start();
    let (_temp, mut a, mut b) = pair(&server);
    let nid = add(&mut a, "原正文");
    converge(&mut a, &mut b);
    edit(&mut a, &nid, "电脑的新正文");
    let mut n = b.notes.iter().find(|n| n.id == nid).unwrap().clone();
    n.set_markdown("手机的新正文".into());
    let aid = b
        .import_bytes(b"offline attachment must survive conflict")
        .unwrap();
    n.blocks.push(Block::File {
        id: id(),
        asset: aid.clone(),
        name: "离线附件.txt".into(),
    });
    b.save_note(n).unwrap();
    converge(&mut a, &mut b);
    for store in [&a, &b] {
        let fork = active(store)
            .into_iter()
            .find(|n| n.conflict_of.as_deref() == Some(&nid))
            .unwrap();
        assert!(
            fork.blocks
                .iter()
                .any(|b| matches!(b, Block::File { asset, .. } if asset == &aid))
        );
        assert_eq!(
            store.asset(&aid).unwrap(),
            b"offline attachment must survive conflict"
        );
    }
}

#[test]
fn direct_account_login_two_devices_without_browser() {
    // 同一 Casdoor 账号的不同设备取得同一同步密钥。
    let server = Server::start();
    let temp = tempfile::tempdir().unwrap();
    let mut a = test_store::open_store(temp.path().join("direct-a")).unwrap();
    let mut b = test_store::open_store(temp.path().join("direct-b")).unwrap();
    let cancel = || std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    assert!(sync::login(&server.url, "alice", "incorrect", cancel()).is_err());
    let connection = sync::login(&server.url, "alice", "login123", cancel()).unwrap();
    sync::connect_account(&mut a, connection).unwrap();
    let nid = add(&mut a, "账号密码登录后的双设备同步");
    sync::once(&mut a).unwrap();
    let connection = sync::login(&server.url, "alice", "login123", cancel()).unwrap();
    sync::connect_account(&mut b, connection).unwrap();
    sync::once(&mut b).unwrap();
    assert_eq!(*a.key.0, *b.key.0);
    assert_eq!(
        b.notes.iter().find(|n| n.id == nid).unwrap().text(),
        "账号密码登录后的双设备同步"
    );
    let session = sync::config(&a).unwrap().unwrap().session;
    let client = reqwest::blocking::Client::new();
    let renewed = auth::refresh(&client, &session).unwrap();
    assert!(auth::refresh(&client, &session).is_err());
    auth::revoke(&client, &renewed).unwrap();
    assert!(auth::refresh(&client, &renewed).is_err());
}
