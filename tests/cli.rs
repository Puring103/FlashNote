#![cfg(windows)]

use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use suojian::{
    model::{AudioState, Block, Note, id},
    store::Store,
};

fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flashnote"))
        .arg("--data-dir")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> Value {
    let output = invoke(root, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_search_pages_and_returns_readable_attachment_paths() {
    let temp = tempfile::Builder::new()
        .prefix("闪记 CLI ")
        .tempdir()
        .unwrap();
    let root = temp.path();
    let mut store = Store::open(root.into()).unwrap();
    let group = store.create_group_with_color("工作", 0x245356).unwrap();
    let bytes = b"\x00\x01\xff attachment";
    let asset = store.import_bytes(bytes).unwrap();
    let mut transcript = Note::new();
    transcript.group_id = group.id;
    transcript.set_markdown("本次会议记录".into());
    transcript.blocks.push(Block::Audio {
        id: id(),
        asset: asset.clone(),
        duration_ms: 100,
        text: Some("下周的项目计划".into()),
        state: AudioState::Ready,
    });
    transcript.blocks.push(Block::File {
        id: id(),
        asset: id(),
        name: "尚未同步.txt".into(),
    });
    store.save_note(transcript.clone()).unwrap();
    let mut exact = Note::new();
    exact.title = "下周计划".into();
    store.save_note(exact.clone()).unwrap();
    let mut typo = Note::new();
    typo.set_markdown("记录了同步失败，并测试了 ＡＰＩ。".into());
    store.save_note(typo.clone()).unwrap();
    for _ in 0..20 {
        let mut note = Note::new();
        note.title = "其它记录".into();
        store.save_note(note).unwrap();
    }
    assert_eq!(invoke(root, &["list"]).status.code(), Some(4));
    drop(store);

    let page = success(root, &["list"]);
    assert_eq!(page, success(root, &["notes", "list"]));
    assert_eq!(page["items"].as_array().unwrap().len(), 20);
    let cursor = page["next_cursor"].as_str().unwrap();
    let rest = success(root, &["list", "--cursor", cursor]);
    assert_eq!(rest["items"].as_array().unwrap().len(), 3);
    assert!(rest["next_cursor"].is_null());
    assert_eq!(
        invoke(root, &["list", "--limit", "0"]).status.code(),
        Some(2)
    );
    assert!(
        success(root, &["list", "--since", "2999-01-01"])["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let matches = success(root, &["search", "下周计划"]);
    assert_eq!(matches["items"][0]["id"], exact.id);
    assert_eq!(matches["items"][1]["matched_field"], "transcript");
    for (scope, expected) in [
        ("title", Some(exact.id.as_str())),
        ("transcript", Some(transcript.id.as_str())),
        ("body", None),
    ] {
        let result = success(root, &["search", "下周计划", "--in", scope]);
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), usize::from(expected.is_some()));
        assert_eq!(items.first().and_then(|note| note["id"].as_str()), expected);
    }
    let body_matches = success(root, &["search", "本次会议", "--in", "body", "--exact"]);
    assert_eq!(body_matches["items"][0]["id"], transcript.id);
    let filtered = success(
        root,
        &[
            "search",
            "计划 下周",
            "--group",
            "工作",
            "--limit",
            "1",
            "--full",
        ],
    );
    let note = &filtered["items"][0];
    assert_eq!(note["id"], transcript.id);
    assert_eq!(note["snippet"], "下周的项目计划");
    let body = note["markdown"].as_str().unwrap();
    assert!(body.contains("file:///"));
    assert!(body.contains(&asset));
    assert_eq!(
        fs::read(note["attachments"][0]["local_path"].as_str().unwrap()).unwrap(),
        bytes
    );
    assert!(note["attachments"][1]["local_path"].is_null());
    assert_eq!(
        success(root, &["get", &transcript.id])["markdown"],
        note["markdown"]
    );
    let batch = success(root, &["get", &transcript.id, &exact.id]);
    assert_eq!(batch["items"].as_array().unwrap().len(), 2);
    assert_eq!(batch["items"][0]["id"], transcript.id);
    assert_eq!(batch["items"][0]["attachments"], note["attachments"]);
    assert_eq!(batch["items"][1]["id"], exact.id);
    let missing = id();
    let failed = invoke(root, &["get", &transcript.id, &missing]);
    assert_eq!(failed.status.code(), Some(3));
    assert!(failed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&failed.stderr).contains(&missing));
    assert_eq!(
        success(root, &["attachments", "path", &asset])["local_path"],
        note["attachments"][0]["local_path"]
    );
    let destination = root.join("导出的录音.wav");
    let export = [
        "attachments",
        "export",
        &asset,
        "--out",
        destination.to_str().unwrap(),
    ];
    success(root, &export);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(invoke(root, &export).status.code(), Some(1));
    for query in ["同步失改", "api"] {
        assert_eq!(success(root, &["search", query])["items"][0]["id"], typo.id);
    }
    assert!(
        success(root, &["search", "同步失改", "--exact"])["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let body_path = root.join("新版.md");
    fs::write(&body_path, "修改后的正文").unwrap();
    let revision = note["revision"].as_str().unwrap();
    let update = [
        "update",
        &transcript.id,
        "--if-revision",
        revision,
        "--file",
        body_path.to_str().unwrap(),
        "--title",
        "新标题",
    ];
    let updated = success(root, &update);
    assert_eq!(updated["id"], transcript.id);
    assert_ne!(updated["revision"], revision);
    assert_eq!(invoke(root, &update).status.code(), Some(5));
    let replaced = success(root, &["get", &transcript.id]);
    assert_eq!(replaced["title"], "新标题");
    assert_eq!(replaced["attachments"], note["attachments"]);
    assert!(
        replaced["markdown"]
            .as_str()
            .unwrap()
            .starts_with("修改后的正文\n\n")
    );
    assert!(
        !replaced["markdown"]
            .as_str()
            .unwrap()
            .contains("本次会议记录")
    );
    let new_revision = replaced["revision"].as_str().unwrap();
    success(
        root,
        &[
            "update",
            &transcript.id,
            "--if-revision",
            new_revision,
            "--group",
            "默认",
        ],
    );
    let moved = success(root, &["get", &transcript.id]);
    assert_eq!(moved["group"], "默认");
    assert_eq!(moved["markdown"], replaced["markdown"]);
}

#[test]
fn cli_writes_to_the_owner_store_and_rejects_stale_revisions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = root.join("body.md");
    fs::write(&file, "离线创建").unwrap();
    let body = file.to_str().unwrap();
    let created = success(root, &["create", "--file", body]);
    let note_id = created["id"].as_str().unwrap();
    let revision = created["revision"].as_str().unwrap();
    let mut store = Store::open(root.into()).unwrap();
    let jobs = suojian::cli_pipe::listen(root).unwrap();

    fs::write(&file, "托盘模式追加").unwrap();
    let append = ["append", note_id, "--if-revision", revision, "--file", body];
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| invoke(root, &append));
        let (request, send) = match jobs.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(job) => job,
            Err(error) => {
                let output = worker.join().unwrap();
                panic!("{error}: {}", String::from_utf8_lossy(&output.stderr));
            }
        };
        assert!(request.writes_store());
        send.send(suojian::cli::execute(&mut store, request))
            .unwrap();
        let output = worker.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let worker = scope.spawn(|| invoke(root, &append));
        let (request, send) = jobs
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        send.send(suojian::cli::execute(&mut store, request))
            .unwrap();
        assert_eq!(worker.join().unwrap().status.code(), Some(5));

        let worker = scope.spawn(|| success(root, &["open", note_id]));
        let (request, send) = jobs
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(&request, suojian::cli::Request::Open { id } if id == note_id));
        send.send(suojian::cli::execute(&mut store, request))
            .unwrap();
        assert_eq!(worker.join().unwrap()["id"], note_id);
    });
    let saved = store.notes.iter().find(|note| note.id == note_id).unwrap();
    assert_eq!(saved.editor_markdown(), "离线创建\n\n托盘模式追加");
}

#[test]
fn groups_attachments_templates_and_portable_export() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let group = success(root, &["groups", "create", "项目 A", "--color", "#123456"]);
    let gid = group["id"].as_str().unwrap();
    assert_eq!(group["color"], "#123456");
    let updated = success(
        root,
        &[
            "groups", "update", gid, "--name", "项目 B", "--color", "abcdef",
        ],
    );
    assert_eq!(updated["name"], "项目 B");
    assert_eq!(updated["color"], "#ABCDEF");
    let destination = success(root, &["groups", "create", "归档"]);
    let destination = destination["id"].as_str().unwrap();

    let one = root.join("记录.txt");
    let two = root.join("议程.txt");
    fs::write(&one, b"first attachment").unwrap();
    fs::write(&two, b"second attachment").unwrap();
    let one = one.to_str().unwrap();
    let two = two.to_str().unwrap();
    let created = success(
        root,
        &[
            "create",
            "--title",
            "会议",
            "--template",
            "meeting",
            "--group",
            gid,
            "--attach",
            one,
        ],
    );
    let note_id = created["id"].as_str().unwrap();
    let revision = created["revision"].as_str().unwrap();
    let missing = root.join("missing.txt");
    let failed = invoke(
        root,
        &[
            "append",
            note_id,
            "--attach",
            two,
            "--attach",
            missing.to_str().unwrap(),
        ],
    );
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(fs::read_dir(root.join("assets")).unwrap().count(), 1);
    assert_eq!(success(root, &["get", note_id])["revision"], revision);
    success(
        root,
        &[
            "append",
            note_id,
            "--if-revision",
            revision,
            "--attach",
            two,
        ],
    );
    let note = success(root, &["get", note_id]);
    assert!(note["markdown"].as_str().unwrap().contains("## 结论"));
    assert_eq!(note["attachments"].as_array().unwrap().len(), 2);

    let file_only = success(root, &["create", "--group", gid, "--attach", one]);
    let trash_id = file_only["id"].as_str().unwrap();
    let mut store = Store::open(root.into()).unwrap();
    let mut trash = store
        .notes
        .iter()
        .find(|note| note.id == trash_id)
        .unwrap()
        .clone();
    trash.deleted_at = Some(suojian::model::now());
    store.save_note(trash).unwrap();
    drop(store);
    let deleted = success(root, &["groups", "delete", gid, "--move-to", destination]);
    assert_eq!(deleted["moved"], 2);
    let store = Store::open(root.into()).unwrap();
    assert!(!store.groups.iter().any(|group| group.id == gid));
    assert!(
        store
            .notes
            .iter()
            .filter(|note| [note_id, trash_id].contains(&note.id.as_str()))
            .all(|n| n.group_id == destination)
    );
    drop(store);

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let items = success(root, &["list", "--since", &today, "--until", &today]);
    assert_eq!(items["items"][0]["id"], note_id);
    assert!(
        success(root, &["list", "--until", "2000-01-01"])["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        invoke(root, &["list", "--since", &today, "--until", "2000-01-01"])
            .status
            .code(),
        Some(2)
    );

    let out = root.join("便携导出");
    let args = ["export", note_id, note_id, "--out", out.to_str().unwrap()];
    let exported = success(root, &args);
    assert_eq!(exported["notes"], 1);
    assert_eq!(exported["attachments"], 2);
    let markdown = fs::read_to_string(out.join(format!("{note_id}.md"))).unwrap();
    assert!(!markdown.contains("file:///"));
    for (item, bytes) in note["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .zip([b"first attachment".as_slice(), b"second attachment"])
    {
        let relative = format!("assets/{}.txt", item["id"].as_str().unwrap());
        assert!(markdown.contains(&relative));
        assert_eq!(fs::read(out.join(relative)).unwrap(), bytes);
    }
    assert_eq!(invoke(root, &args).status.code(), Some(2));
}
