//! Headless companion. Uses the same Store and sync implementation as the GUI.
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use suojian::{model::*, store::Store, sync};
#[path = "fixtures/mod.rs"]
mod fixtures;
#[cfg(test)]
#[path = "../../../../tests/support/test_store.rs"]
mod test_store;

fn demo(root: PathBuf, url: &str) -> Result<()> {
    anyhow::ensure!(
        !root.exists() || std::fs::read_dir(&root)?.next().is_none(),
        "演示目录必须为空"
    );
    let mut a = Store::open(root.join("device-a"))?;
    login(&mut a, url)?;
    let mut shared = Note::new();
    shared
        .set_markdown("电脑离线后会修改这里\n\n中间的内容保持不变\n\n手机离线后会修改这里".into());
    let shared_id = shared.id.clone();
    a.save_note(shared)?;
    let mut conflict = Note::new();
    conflict.set_markdown("这行会分别被两台离线设备修改".into());
    let conflict_id = conflict.id.clone();
    a.save_note(conflict)?;
    sync::once(&mut a)?;
    let mut b = Store::open(root.join("device-b"))?;
    login(&mut b, url)?;
    sync::once(&mut b)?;
    for (store, merged_text, conflicting_text, new_text, attachment_name, attachment) in [
        (
            &mut a,
            "电脑离线新增的第一段\n\n中间的内容保持不变\n\n手机离线后会修改这里",
            "同一行的电脑版本：已经完整保留",
            "电脑离线新建的便签，附带文件。",
            "电脑离线附件.txt",
            b"offline attachment from device A".to_vec(),
        ),
        (
            &mut b,
            "电脑离线后会修改这里\n\n中间的内容保持不变\n\n手机离线新增的最后一段",
            "同一行的手机版本：作为离线冲突副本保留",
            "手机离线新建的便签，附带图片。",
            "phone-photo.jpg",
            include_bytes!("../../../../assets/demo/desk.jpg").to_vec(),
        ),
    ] {
        for (nid, text) in [(&shared_id, merged_text), (&conflict_id, conflicting_text)] {
            let mut n = store.notes.iter().find(|n| &n.id == nid).unwrap().clone();
            n.set_markdown(text.into());
            store.save_note(n)?;
        }
        let mut n = Note::new();
        n.set_markdown(new_text.into());
        let nid = n.id.clone();
        store.save_note(n)?;
        let prepared = suojian::media::prepare_bytes(attachment_name.into(), attachment, false)?;
        store.attach_prepared(&nid, prepared)?;
    }
    let mut reports = vec![];
    for _ in 0..3 {
        reports.push(sync::once(&mut a)?);
        reports.push(sync::once(&mut b)?);
    }
    for store in [&a, &b] {
        std::fs::write(store.root.join("samples-v3.done"), b"isolated sync demo")?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "device_a": a.root, "device_b": b.root, "notes_on_each_device": a.notes.len(),
            "merged": reports.iter().map(|r|r.merged).sum::<usize>(),
            "conflict_copies": reports.iter().map(|r|r.conflicts).sum::<usize>(),
            "usage": sync::usage(&a)?
        }))?
    );
    Ok(())
}

fn login(store: &mut Store, url: &str) -> Result<()> {
    let username = std::env::var("SUOJIAN_SYNC_USERNAME")
        .context("请设置 SUOJIAN_SYNC_USERNAME（登录账号）")?;
    let secret = zeroize::Zeroizing::new(
        std::env::var("SUOJIAN_SYNC_LOGIN_SECRET")
            .context("请设置 SUOJIAN_SYNC_LOGIN_SECRET（登录密码）")?,
    );
    let connection = sync::login(url, &username, &secret, Arc::new(AtomicBool::new(false)))?;
    sync::connect_account(store, connection)
}

fn main() -> Result<()> {
    #[cfg(target_os = "linux")]
    suojian::linux_keyring::install()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let root: PathBuf = std::env::var("SUOJIAN_DATA_DIR")
        .context("同步工具要求显式设置 SUOJIAN_DATA_DIR，避免误操作正式数据")?
        .into();
    let Some(command) = args.first().map(String::as_str) else {
        bail!(
            "用法: suojian-sync login <URL> | logout | once | status | demo <URL> | cases | verify-demo"
        );
    };
    if command == "demo" {
        return demo(root, args.get(1).context("缺少服务器地址")?);
    }
    if command == "cases" {
        return fixtures::install(&root);
    }
    if command == "verify-demo" {
        return fixtures::verify(&root);
    }
    let mut store = Store::open(root)?;
    match command {
        "login" => {
            login(&mut store, args.get(1).context("缺少服务器地址")?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&sync::once(&mut store)?)?
            );
        }
        "logout" => {
            if let Some(config) = sync::logout(&store)? {
                sync::revoke_session(&config)?;
            }
            println!("已退出同步账号，本地便签保留");
        }
        "once" => println!(
            "{}",
            serde_json::to_string_pretty(&sync::once(&mut store)?)?
        ),
        "status" => println!("{}", serde_json::to_string_pretty(&sync::usage(&store)?)?),
        _ => bail!("未知命令"),
    }
    Ok(())
}
