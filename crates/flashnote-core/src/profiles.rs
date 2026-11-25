//! The signed-out collection and each account own separate Store directories.
use crate::{
    archive, local,
    model::*,
    store::{Store, StoreBusy, write_object},
    sync,
};
use anyhow::{Context, Result, ensure};
use rusqlite::OptionalExtension;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

fn guest(base: &Path) -> PathBuf {
    if base.join("sync-owner.json").exists() {
        base.join("local")
    } else {
        base.to_owned()
    }
}

pub fn open(base: PathBuf) -> Result<Store> {
    fs::create_dir_all(&base)?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(base.join("profiles.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| StoreBusy)?;
    let active = base.join("active-account");
    let mut dir = guest(&base);
    if active.exists() {
        let id = uuid::Uuid::parse_str(&fs::read_to_string(&active)?)?;
        let account = base.join("accounts").join(id.to_string());
        if account.join("sync-session.dpapi").exists() {
            dir = account;
        }
    }
    let mut store = Store::open(dir)?;
    store.profile = Some((base, Arc::new(lock)));
    Ok(store)
}

pub fn credentials(store: &Store) -> Result<PathBuf> {
    let Some((base, _)) = &store.profile else {
        return Ok(store.root.clone());
    };
    let path = base.join("account-keys");
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn connect(store: &mut Store, connection: sync::Connection) -> Result<()> {
    let Some(profile) = store.profile.clone() else {
        return sync::connect_account(store, connection);
    };
    let cfg = &connection.config;
    let identity = serde_json::to_vec(&(&cfg.url, &cfg.issuer, &cfg.subject, &cfg.vault_id))?;
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, &identity).to_string();
    let target = profile.0.join("accounts").join(&id);
    if target == store.root {
        return sync::connect_account(store, connection);
    }
    let mut account = Store::open(target)?;
    if cfg.e2ee {
        let file = format!(
            "recovery-{}.protected",
            uuid::Uuid::parse_str(&cfg.vault_id)?
        );
        local::atomic_write(
            &account.root.join(&file),
            &fs::read(credentials(store)?.join(file))?,
        )?;
    }
    sync::connect_account(&mut account, connection)?;
    local::atomic_write(&profile.0.join("active-account"), id.as_bytes())?;
    account.profile = Some(profile);
    *store = account;
    Ok(())
}

pub fn logout(store: &mut Store) -> Result<Option<sync::Config>> {
    let Some(profile) = store.profile.clone() else {
        return sync::logout(store);
    };
    let dir = guest(&profile.0);
    if dir == store.root {
        return sync::logout(store);
    }
    let mut local = Store::open(dir)?;
    let session = sync::logout(store)?;
    local.profile = Some(profile);
    *store = local;
    Ok(session)
}

pub fn has_local(store: &Store) -> Result<bool> {
    let Some((base, _)) = &store.profile else {
        return Ok(false);
    };
    let dir = guest(base);
    if dir == store.root {
        return Ok(store.notes.iter().any(|n| !n.is_empty()) || store.groups.len() > 1);
    }
    let db = dir.join("notes.sqlite3");
    if !db.exists() {
        return Ok(false);
    }
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE (kind='note' AND (json_extract(payload,'$.title')<>'' OR json_array_length(payload,'$.blocks')>0)) OR (kind='group' AND id<>? AND NOT json_extract(payload,'$.deleted')))", [DEFAULT_GROUP], |r| r.get(0))?)
}

pub fn import_local(target: &mut Store) -> Result<()> {
    ensure!(sync::config(target)?.is_some(), "请先登录账号");
    let base = &target.profile.as_ref().context("缺少本机资料目录")?.0;
    let mut source = Store::open(guest(base))?;
    let notes: Vec<_> = source
        .notes
        .iter()
        .filter(|n| !n.is_empty())
        .cloned()
        .collect();
    for aid in archive::assets(&notes) {
        let bytes = source.asset(&aid)?;
        let path = target.root.join("assets").join(aid);
        if path.exists() {
            ensure!(fs::read(path)? == bytes, "附件 ID 冲突");
        } else {
            local::atomic_write(&path, &bytes)?;
        }
    }
    let tx = target.db.transaction()?;
    let mut added_groups = Vec::new();
    let mut added_notes = Vec::new();
    for group in source.groups.iter().filter(|g| g.id != DEFAULT_GROUP) {
        if let Some(existing) = target.groups.iter().find(|g| g.id == group.id) {
            ensure!(existing == group, "分组 ID 冲突");
            continue;
        }
        write_object(&tx, &target.device, &group.id, "group", &id(), None, group)?;
        added_groups.push(group.clone());
    }
    for note in &notes {
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM sync_state WHERE key=?",
                [format!("import/{}", note.id)],
                |r| r.get(0),
            )
            .optional()?;
        if previous.as_ref() == Some(&note.revision) {
            continue;
        }
        ensure!(
            !target.notes.iter().any(|n| n.id == note.id),
            "便签 ID 冲突，原件已保留"
        );
        write_object(
            &tx,
            &target.device,
            &note.id,
            "note",
            &note.revision,
            None,
            note,
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state VALUES(?,?)",
            [&format!("import/{}", note.id), &note.revision],
        )?;
        added_notes.push(note.clone());
    }
    tx.commit()?;
    target.notes.extend(added_notes);
    target.groups.extend(added_groups);
    target.sort_groups();
    let tx = source.db.transaction()?;
    for table in ["sync_packets", "outbox", "revisions", "objects"] {
        let sql = match table {
            "sync_packets" | "outbox" => format!("DELETE FROM {table} WHERE revision IN (SELECT revision FROM revisions WHERE object_id IN (SELECT id FROM objects WHERE kind IN ('note','group') AND id<>?))"),
            "revisions" => "DELETE FROM revisions WHERE object_id IN (SELECT id FROM objects WHERE kind IN ('note','group') AND id<>?)".into(),
            _ => "DELETE FROM objects WHERE kind IN ('note','group') AND id<>?".into()
        };
        tx.execute(&sql, [DEFAULT_GROUP])?;
    }
    tx.commit()?;
    Ok(())
}
