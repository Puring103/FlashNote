use crate::{
    crypto::{VaultKey, protect},
    model::*,
};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::PathBuf;

#[derive(Debug)]
pub struct StoreBusy;

impl std::fmt::Display for StoreBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("此资料目录已被另一个进程打开，请先退出使用它的闪记或同步工具")
    }
}

impl std::error::Error for StoreBusy {}

#[derive(Clone, Copy, Debug)]
pub enum ConflictChoice {
    Local,
    Cloud,
    Both,
}

pub struct Store {
    pub(crate) db: Connection,
    pub key: VaultKey,
    pub root: PathBuf,
    pub groups: Vec<Group>,
    pub notes: Vec<Note>,
    pub settings: Settings,
    pub device: String,
    // Drop this last: one owner process also serializes token rotation.
    pub(crate) profile: Option<(PathBuf, std::sync::Arc<std::fs::File>)>,
    _directory_lock: std::fs::File,
}
impl Store {
    pub fn remote_image_error(&self, block_id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM sync_state WHERE key=?",
                [format!("remote-image-{block_id}")],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_remote_image_error(&self, block_id: &str, error: Option<&str>) -> Result<()> {
        let key = format!("remote-image-{block_id}");
        match error {
            Some(error) => self.db.execute(
                "INSERT OR REPLACE INTO sync_state VALUES(?,?)",
                params![key, error],
            )?,
            None => self
                .db
                .execute("DELETE FROM sync_state WHERE key=?", [key])?,
        };
        Ok(())
    }

    pub fn conflict_pair(&self, nid: &str) -> Option<(Note, Note)> {
        let fork = self.notes.iter().find(|n| {
            n.deleted_at.is_none()
                && !n.conflict_resolved
                && n.conflict_of.is_some()
                && (n.id == nid || n.conflict_of.as_deref() == Some(nid))
        })?;
        let cloud = self
            .notes
            .iter()
            .find(|n| Some(&n.id) == fork.conflict_of.as_ref())?;
        Some((fork.clone(), cloud.clone()))
    }

    /// Both revisions are checked again at the instant of the decision. All
    /// resulting revisions/outbox entries commit together; history is retained.
    pub fn resolve_conflict(
        &mut self,
        fork_id: &str,
        fork_revision: &str,
        cloud_revision: &str,
        choice: ConflictChoice,
    ) -> Result<()> {
        let (mut fork, cloud) = self
            .conflict_pair(fork_id)
            .ok_or_else(|| anyhow::anyhow!("此冲突已处理或便签已删除"))?;
        anyhow::ensure!(
            fork.revision == fork_revision && cloud.revision == cloud_revision,
            "版本已更新，请重新比较后选择"
        );
        let mut changed = vec![];
        if matches!(choice, ConflictChoice::Local) {
            let mut selected = fork.clone();
            selected.id = cloud.id.clone();
            selected.conflict_of = cloud.conflict_of.clone();
            selected.conflict_resolved = cloud.conflict_resolved;
            selected.created_at = cloud.created_at;
            selected.parent_revision = Some(cloud.revision.clone());
            selected.revision = id();
            selected.updated_at = now();
            changed.push(selected);
        }
        fork.conflict_resolved = true;
        fork.parent_revision = Some(fork.revision.clone());
        fork.revision = id();
        fork.updated_at = now();
        if !matches!(choice, ConflictChoice::Both) {
            fork.deleted_at = Some(now());
        }
        changed.push(fork);
        let tx = self.db.transaction()?;
        for note in &changed {
            write_object(
                &tx,
                &self.device,
                &note.id,
                "note",
                &note.revision,
                note.parent_revision.as_deref(),
                note,
            )?;
        }
        tx.commit()?;
        for note in changed {
            self.notes.retain(|n| n.id != note.id);
            self.notes.push(note);
        }
        Ok(())
    }
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(root.join("assets"))?;
        let directory_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("store.lock"))?;
        fs2::FileExt::try_lock_exclusive(&directory_lock).map_err(|_| StoreBusy)?;
        let key_path = root.join("vault-key.dpapi");
        let key = if key_path.exists() {
            let bytes = protect(&std::fs::read(key_path)?, true)?;
            if bytes.len() != 32 {
                bail!("密钥长度错误");
            }
            let mut k = [0; 32];
            k.copy_from_slice(&bytes);
            VaultKey(zeroize::Zeroizing::new(k))
        } else {
            if root.join("notes.sqlite3").exists() {
                bail!("本机密钥丢失，无法打开资料库，不会覆盖数据库");
            }
            let k = VaultKey::random();
            std::fs::write(key_path, protect(&*k.0, false)?)?;
            k
        };
        let db = Connection::open(root.join("notes.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS objects(id TEXT PRIMARY KEY, kind TEXT NOT NULL, revision TEXT NOT NULL, payload BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS revisions(revision TEXT PRIMARY KEY, object_id TEXT NOT NULL, parent TEXT, device TEXT NOT NULL, created_at INTEGER NOT NULL, payload BLOB NOT NULL);
            CREATE INDEX IF NOT EXISTS revisions_by_object ON revisions(object_id, created_at DESC);
            CREATE TABLE IF NOT EXISTS outbox(revision TEXT PRIMARY KEY REFERENCES revisions(revision), acknowledged INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS sync_heads(id TEXT PRIMARY KEY, kind TEXT NOT NULL, revision TEXT NOT NULL, payload BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS sync_packets(revision TEXT PRIMARY KEY, packet TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sync_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        std::fs::create_dir_all(root.join("sync-assets"))?;
        std::fs::create_dir_all(root.join("sync-image-receipts"))?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let empty: bool =
            db.query_row("SELECT NOT EXISTS(SELECT 1 FROM objects)", [], |r| r.get(0))?;
        if version == 0 && empty {
            db.pragma_update(None, "user_version", 3)?;
        } else if version != 3 {
            bail!("不支持的本地数据库格式");
        }
        let device_path = root.join("device-id");
        let device = if device_path.exists() {
            std::fs::read_to_string(device_path)?
        } else {
            let d = id();
            std::fs::write(device_path, &d)?;
            d
        };
        let mut store = Self {
            profile: None,
            _directory_lock: directory_lock,
            db,
            key,
            root,
            device,
            groups: vec![],
            notes: vec![],
            settings: Settings::default(),
        };
        {
            let mut stmt = store
                .db
                .prepare("SELECT id, kind, revision, payload FROM objects")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                ))
            })?;
            for row in rows {
                let (_, kind, _, plain) = row?;
                match kind.as_str() {
                    "note" => store.notes.push(serde_json::from_slice(&plain)?),
                    "group" => store.groups.push(serde_json::from_slice(&plain)?),
                    "settings" => store.settings = serde_json::from_slice(&plain)?,
                    _ => {}
                }
            }
        }
        if !store.groups.iter().any(|g| g.id == DEFAULT_GROUP) {
            store.save_group(Group {
                order: 0,
                deleted: false,
                id: DEFAULT_GROUP.into(),
                name: "默认".into(),
                color: 0x7c8179,
                icon: default_group_icon(),
            })?;
        }
        store.sort_groups();
        let external: Vec<_> = store
            .notes
            .iter()
            .filter(|n| {
                n.blocks.iter().any(
                    |block| matches!(block, Block::Markdown { text, .. } if text.contains("![")),
                )
            })
            .filter(|n| crate::remote_images::contains(&n.editor_markdown()))
            .cloned()
            .collect();
        for note in external {
            store.save_note(note)?;
        }
        Ok(store)
    }
    pub(crate) fn sort_groups(&mut self) {
        self.groups.retain(|g| !g.deleted || g.id == DEFAULT_GROUP);
        self.groups.sort_by(|a, b| {
            (a.id != DEFAULT_GROUP)
                .cmp(&(b.id != DEFAULT_GROUP))
                .then_with(|| a.order.cmp(&b.order))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    fn persist(
        &mut self,
        oid: &str,
        kind: &str,
        rev: &str,
        parent: Option<&str>,
        data: &impl serde::Serialize,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        write_object(&tx, &self.device, oid, kind, rev, parent, data)?;
        tx.commit()?;
        Ok(())
    }
    pub fn save_note(&mut self, mut n: Note) -> Result<()> {
        if let Some(existing) = self.notes.iter().find(|existing| existing.id == n.id) {
            anyhow::ensure!(
                n.revision == existing.revision,
                "便签版本已变化，请刷新后保存；未覆盖现有内容"
            );
        }
        let source = n.editor_markdown();
        if crate::remote_images::contains(&source) {
            n.set_editor_markdown(&source);
        }
        if let Some(existing) = self.notes.iter().find(|existing| existing.id == n.id) {
            if serde_json::to_value(existing)? == serde_json::to_value(&n)? {
                return Ok(());
            }
        }
        if n.is_empty() {
            let exists: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM objects WHERE id=?1)",
                [&n.id],
                |r| r.get(0),
            )?;
            if exists {
                n.parent_revision = Some(n.revision.clone());
                n.deleted_at = Some(now());
                n.updated_at = now();
                n.revision = id();
                self.persist(&n.id, "note", &n.revision, n.parent_revision.as_deref(), &n)?;
            }
            self.notes.retain(|x| x.id != n.id);
            self.notes.push(n);
            return Ok(());
        }
        if !self.groups.iter().any(|g| g.id == n.group_id) {
            n.group_id = DEFAULT_GROUP.into();
        }
        n.parent_revision = self
            .notes
            .iter()
            .find(|x| x.id == n.id)
            .map(|x| x.revision.clone());
        n.revision = id();
        n.updated_at = now();
        self.persist(&n.id, "note", &n.revision, n.parent_revision.as_deref(), &n)?;
        self.notes.retain(|x| x.id != n.id);
        self.notes.push(n);
        Ok(())
    }
    /// Commit against the document actually observed by an editor, preserving
    /// newer metadata and merging independent edits from other local views.
    pub fn save_editor(
        &mut self,
        nid: &str,
        base_revision: &str,
        base_source: &str,
        source: &str,
    ) -> Result<Note> {
        let mut note = self
            .notes
            .iter()
            .find(|n| n.id == nid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("便签已不存在，编辑内容仍保留在窗口中"))?;
        let latest = note.editor_markdown();
        let mut incoming = note.clone();
        incoming.set_editor_markdown(source);
        let incoming_source = incoming.editor_markdown();
        if note.revision != base_revision && incoming_source != latest {
            anyhow::ensure!(
                note.deleted_at.is_none(),
                "便签已被删除，编辑内容仍保留在窗口中，请复制后另存"
            );
            let merged = diffy::merge(base_source, &incoming_source, &latest).map_err(|_| {
                anyhow::anyhow!(
                    "其他窗口修改了相同位置，未覆盖任何版本；当前编辑仍保留，请复制后另存"
                )
            })?;
            incoming.set_editor_markdown(&merged);
            note = incoming;
        } else {
            note = incoming;
        }
        // Continuing to type into a cleared note restores the same note with
        // the persisted tombstone as its parent revision.
        if note.revision == base_revision && !note.is_empty() {
            note.deleted_at = None;
        }
        if note.editor_markdown() != latest
            || note.deleted_at != self.notes.iter().find(|n| n.id == nid).unwrap().deleted_at
        {
            self.save_note(note)?;
        }
        Ok(self.notes.iter().find(|n| n.id == nid).unwrap().clone())
    }
    pub fn complete_transcript(
        &mut self,
        nid: &str,
        bid: &str,
        result: std::result::Result<String, String>,
    ) -> Result<bool> {
        let Some(mut note) = self
            .notes
            .iter()
            .find(|n| n.id == nid && n.deleted_at.is_none())
            .cloned()
        else {
            return Ok(false);
        };
        let Some(Block::Audio { text, state, .. }) = note
            .blocks
            .iter_mut()
            .chain(note.media_archive.iter_mut())
            .find(|b| b.id() == bid)
        else {
            return Ok(false);
        };
        if *state != AudioState::Processing {
            return Ok(false);
        }
        match result {
            Ok(value) => {
                *text = Some(value);
                *state = AudioState::Ready;
            }
            Err(error) => *state = AudioState::Failed(error),
        }
        self.save_note(note)?;
        Ok(true)
    }
    #[cfg(test)]
    pub fn create_group(&mut self, name: &str) -> Result<Group> {
        self.create_group_with_color(name, 0x245356)
    }
    pub fn create_group_with_color(&mut self, name: &str, color: u32) -> Result<Group> {
        self.create_group_with_style(name, color, "dot")
    }
    pub fn create_group_with_style(&mut self, name: &str, color: u32, icon: &str) -> Result<Group> {
        anyhow::ensure!(color <= 0xffffff, "无效分组颜色");
        let name = name.trim();
        if name.is_empty() {
            bail!("请输入分组名称");
        }
        if name.chars().count() > 20 {
            bail!("分组名称最多20个字");
        }
        if self
            .groups
            .iter()
            .any(|g| g.name.to_lowercase() == name.to_lowercase())
        {
            bail!("已有同名分组");
        }
        let group = Group {
            order: 0,
            deleted: false,
            id: id(),
            name: name.into(),
            color,
            icon: group_icon(icon).into(),
        };
        self.save_group(group.clone())?;
        Ok(group)
    }
    pub fn save_group(&mut self, g: Group) -> Result<()> {
        anyhow::ensure!(
            !g.name.trim().is_empty() && g.name.chars().count() <= 20 && g.color <= 0xffffff,
            "分组名称或颜色无效"
        );
        anyhow::ensure!(
            !self
                .groups
                .iter()
                .any(|old| old.id != g.id && old.name.to_lowercase() == g.name.to_lowercase()),
            "已有同名分组"
        );
        if g.id == DEFAULT_GROUP && (g.name != "默认" || g.deleted) {
            bail!("默认分组不能重命名");
        }
        let parent: Option<String> = self
            .db
            .query_row("SELECT revision FROM objects WHERE id=?1", [&g.id], |r| {
                r.get(0)
            })
            .optional()?;
        self.persist(&g.id, "group", &id(), parent.as_deref(), &g)?;
        if let Some(existing) = self.groups.iter_mut().find(|x| x.id == g.id) {
            *existing = g;
        } else {
            self.groups.push(g);
        }
        self.sort_groups();
        Ok(())
    }
    pub fn save_settings(&mut self, settings: Settings) -> Result<()> {
        self.persist("settings", "settings", &id(), None, &settings)?;
        self.db.execute("UPDATE outbox SET acknowledged=1 WHERE revision IN (SELECT revision FROM revisions WHERE object_id='settings')", [])?;
        self.settings = settings;
        Ok(())
    }
    pub fn attach_prepared(
        &mut self,
        nid: &str,
        prepared: crate::media::PreparedAttachment,
    ) -> Result<String> {
        let note = self
            .notes
            .iter()
            .find(|n| n.id == nid && n.deleted_at.is_none())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("便签已删除，未导入附件"))?;
        let mut assets = self.save_note_with_attachments(note, [Ok(prepared)])?;
        Ok(assets.pop().unwrap())
    }

    pub fn save_note_with_attachments(
        &mut self,
        mut note: Note,
        attachments: impl IntoIterator<Item = Result<crate::media::PreparedAttachment>>,
    ) -> Result<Vec<String>> {
        let mut assets = Vec::new();
        let result = (|| {
            for prepared in attachments {
                let prepared = prepared?;
                let asset = self.import_bytes(&prepared.bytes)?;
                let block = if prepared.preview.is_some() {
                    Block::Image {
                        id: id(),
                        asset: asset.clone(),
                        name: prepared.name,
                    }
                } else {
                    Block::File {
                        id: id(),
                        asset: asset.clone(),
                        name: prepared.name,
                    }
                };
                assets.push(asset);
                note.blocks.push(block);
            }
            self.save_note(note)
        })();
        if let Err(error) = result {
            for asset in assets {
                let _ = std::fs::remove_file(self.root.join("assets").join(asset));
            }
            return Err(error);
        }
        Ok(assets)
    }
    pub fn import_bytes(&self, bytes: &[u8]) -> Result<String> {
        anyhow::ensure!(
            bytes.len() as u64 <= crate::media::MAX_FILE,
            "附件超过 128 MiB"
        );
        use std::io::Write;
        loop {
            let aid = id();
            let path = self.root.join("assets").join(&aid);
            let mut file = match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            };
            if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = std::fs::remove_file(path);
                return Err(e.into());
            }
            return Ok(aid);
        }
    }
    pub fn asset(&self, aid: &str) -> Result<Vec<u8>> {
        uuid::Uuid::parse_str(aid)?;
        Ok(std::fs::read(self.root.join("assets").join(aid))?)
    }
    /// Decrypted copies are confined to the same vault root; never use attachment names as paths.
    pub fn materialize_attachment(&self, aid: &str, name: &str) -> Result<std::path::PathBuf> {
        uuid::Uuid::parse_str(aid)?;
        let bytes = self.asset(aid)?;
        let extension = std::path::Path::new(name)
            .extension()
            .and_then(|x| x.to_str())
            .filter(|x| {
                !x.is_empty() && x.len() <= 16 && x.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .unwrap_or("bin");
        let directory = self.root.join("cache").join("open");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{aid}.{extension}"));
        std::fs::write(&path, bytes)?;
        Ok(path)
    }

    pub fn history(&self, note_id: &str) -> Result<Vec<Note>> {
        let mut stmt = self.db.prepare("SELECT payload FROM revisions WHERE object_id=?1 ORDER BY created_at DESC, rowid DESC LIMIT 40")?;
        let mut notes = vec![];
        for row in stmt.query_map([note_id], |r| r.get::<_, Vec<u8>>(0))? {
            notes.push(serde_json::from_slice(&row?)?);
        }
        Ok(notes)
    }
}

pub(crate) fn write_object(
    tx: &Transaction<'_>,
    device: &str,
    oid: &str,
    kind: &str,
    rev: &str,
    parent: Option<&str>,
    value: &impl serde::Serialize,
) -> Result<()> {
    let payload = serde_json::to_vec(value)?;
    upsert_object(tx, oid, kind, rev, &payload)?;
    tx.execute(
        "INSERT INTO revisions VALUES(?,?,?,?,?,?)",
        params![rev, oid, parent, device, now(), payload],
    )?;
    tx.execute("INSERT INTO outbox(revision) VALUES(?)", [rev])?;
    Ok(())
}

pub(crate) fn upsert_object(
    tx: &Transaction<'_>,
    oid: &str,
    kind: &str,
    rev: &str,
    payload: &[u8],
) -> Result<()> {
    let written = tx.execute(
        "INSERT INTO objects VALUES(?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload
         WHERE objects.kind=excluded.kind",
        params![oid, kind, rev, payload],
    )?;
    anyhow::ensure!(written == 1, "对象 ID 已被另一种内容占用，未覆盖数据");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clearing_and_continuing_preserves_persisted_revision_chain() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let mut note = Note::new();
        note.set_editor_markdown("正文");
        let nid = note.id.clone();
        store.save_note(note).unwrap();
        let base = store.notes[0].clone();
        let cleared = store
            .save_editor(&nid, &base.revision, &base.editor_markdown(), "")
            .unwrap();
        assert!(cleared.deleted_at.is_some());
        assert_eq!(
            cleared.parent_revision.as_deref(),
            Some(base.revision.as_str())
        );
        let payload: Vec<u8> = store
            .db
            .query_row("SELECT payload FROM objects WHERE id=?", [&nid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::to_value(&cleared).unwrap(),
            serde_json::from_slice::<serde_json::Value>(&payload).unwrap()
        );
        let restored = store
            .save_editor(&nid, &cleared.revision, "", "继续输入")
            .unwrap();
        assert!(restored.deleted_at.is_none());
        assert_eq!(
            restored.parent_revision.as_deref(),
            Some(cleared.revision.as_str())
        );
        drop(store);
        let store = Store::open(temp.path().to_path_buf()).unwrap();
        assert_eq!(store.notes[0].editor_markdown(), "继续输入");
    }

    #[test]
    fn editor_commits_merge_independent_changes_and_reject_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let mut note = Note::new();
        note.set_editor_markdown("first\n\nsecond\n\nthird");
        let nid = note.id.clone();
        store.save_note(note).unwrap();
        let base = store.notes[0].clone();
        let source = base.editor_markdown();
        store
            .save_editor(&nid, &base.revision, &source, "FIRST\n\nsecond\n\nthird")
            .unwrap();
        let merged = store
            .save_editor(&nid, &base.revision, &source, "first\n\nsecond\n\nTHIRD")
            .unwrap();
        assert_eq!(merged.editor_markdown(), "FIRST\n\nsecond\n\nTHIRD");
        let revision = merged.revision.clone();
        assert!(
            store
                .save_editor(
                    &nid,
                    &base.revision,
                    &source,
                    "conflicting\n\nsecond\n\nthird"
                )
                .is_err()
        );
        assert_eq!(store.notes[0].revision, revision);
        assert_eq!(store.notes[0].editor_markdown(), merged.editor_markdown());
        let same = store
            .save_editor(
                &nid,
                &revision,
                &merged.editor_markdown(),
                &merged.editor_markdown(),
            )
            .unwrap();
        assert_eq!(same.revision, revision);
    }

    #[test]
    fn archived_audio_accepts_transcript_once_and_survives_restore() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let mut note = Note::new();
        let nid = note.id.clone();
        let bid = id();
        note.set_editor_markdown("正文");
        note.blocks.push(Block::Audio {
            id: bid.clone(),
            asset: id(),
            duration_ms: 1,
            text: None,
            state: AudioState::Processing,
        });
        let with_audio = note.editor_markdown();
        note.set_editor_markdown("正文");
        store.save_note(note).unwrap();
        assert!(
            store
                .complete_transcript(&nid, &bid, Ok("识别结果".into()))
                .unwrap()
        );
        let revision = store.notes[0].revision.clone();
        assert!(
            !store
                .complete_transcript(&nid, &bid, Ok("旧结果".into()))
                .unwrap()
        );
        assert!(
            !store
                .complete_transcript(&nid, "missing", Ok("忽略".into()))
                .unwrap()
        );
        assert_eq!(store.notes[0].revision, revision);
        let mut note = store.notes[0].clone();
        note.set_editor_markdown(&with_audio);
        assert!(note.blocks.iter().any(|b| matches!(b, Block::Audio {text: Some(text), state: AudioState::Ready, ..} if text == "识别结果")));
    }

    #[test]
    fn editor_keeps_newer_metadata_and_cannot_resurrect_an_external_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let group = store.create_group("工作").unwrap();
        let mut note = Note::new();
        let nid = note.id.clone();
        note.set_editor_markdown("正文");
        store.save_note(note).unwrap();
        let base = store.notes[0].clone();
        let mut latest = base.clone();
        latest.group_id = group.id.clone();
        store.save_note(latest).unwrap();
        let saved = store
            .save_editor(&nid, &base.revision, &base.editor_markdown(), "新正文")
            .unwrap();
        assert_eq!(saved.group_id, group.id);
        assert_eq!(saved.editor_markdown(), "新正文");
        store.save_note(saved.clone()).unwrap();
        assert_eq!(store.notes[0].revision, saved.revision); // no-op writes create no history
        let mut deleted = saved.clone();
        deleted.deleted_at = Some(now());
        store.save_note(deleted).unwrap();
        let revision = store.notes[0].revision.clone();
        assert!(
            store
                .save_editor(
                    &nid,
                    &saved.revision,
                    &saved.editor_markdown(),
                    "未保存修改"
                )
                .is_err()
        );
        assert_eq!(store.notes[0].revision, revision);
        assert!(store.notes[0].deleted_at.is_some());
    }
    #[test]
    fn group_creation_validates_names_without_duplicates() {
        let mut store =
            Store::open(std::env::temp_dir().join(format!("suojian-groups-{}", id()))).unwrap();
        assert!(store.create_group(" ").is_err());
        assert!(store.create_group("默认").is_err());
        let group = store.create_group("  项目A  ").unwrap();
        assert_eq!(group.name, "项目A");
        assert!(store.create_group("项目a").is_err());
        assert!(store.create_group(&"长".repeat(21)).is_err());
        assert_eq!(store.groups.len(), 2);
    }
    #[test]
    fn attachment_keeps_latest_text_and_rolls_back_failed_save() {
        let root = std::env::temp_dir().join(format!("suojian-attach-{}", id()));
        let mut store = Store::open(root).unwrap();
        let mut n = Note::new();
        let nid = n.id.clone();
        n.title = "最新文字".into();
        store.save_note(n).unwrap();
        let prepared =
            crate::media::prepare_bytes("file.txt".into(), b"attachment".to_vec(), false).unwrap();
        let asset = store.attach_prepared(&nid, prepared).unwrap();
        assert_eq!(store.asset(&asset).unwrap(), b"attachment");
        let note = store.notes.iter().find(|n| n.id == nid).unwrap();
        assert_eq!(note.title, "最新文字");
        assert!(matches!(note.blocks.last(), Some(Block::File { .. })));
        let before = std::fs::read_dir(store.root.join("assets"))
            .unwrap()
            .count();
        store.db.execute_batch("CREATE TRIGGER reject_revision BEFORE INSERT ON revisions BEGIN SELECT RAISE(FAIL,'test failure'); END;").unwrap();
        let p = crate::media::prepare_bytes("other.txt".into(), b"x".to_vec(), false).unwrap();
        assert!(store.attach_prepared(&nid, p).is_err());
        assert_eq!(
            std::fs::read_dir(store.root.join("assets"))
                .unwrap()
                .count(),
            before
        );
        assert_eq!(
            store
                .notes
                .iter()
                .find(|n| n.id == nid)
                .unwrap()
                .blocks
                .len(),
            2
        );
        store
            .db
            .execute_batch("DROP TRIGGER reject_revision;")
            .unwrap();
        let mut n = store.notes.iter().find(|n| n.id == nid).unwrap().clone();
        n.deleted_at = Some(now());
        store.save_note(n).unwrap();
        let p = crate::media::prepare_bytes("x.txt".into(), b"x".to_vec(), false).unwrap();
        assert!(store.attach_prepared(&nid, p).is_err());
    }
    #[test]
    fn persists_plain_local_notes_and_outbox() {
        let root = std::env::temp_dir().join(format!("suojian-test-{}", id()));
        let mut s = Store::open(root.clone()).unwrap();
        let mut n = Note::new();
        n.title = "private-title-unique".into();
        n.set_markdown("private-body-unique".into());
        let nid = n.id.clone();
        s.save_note(n).unwrap();
        let mut n = s.notes[0].clone();
        n.set_markdown("second revision".into());
        s.save_note(n).unwrap();
        assert_eq!(s.history(&nid).unwrap().len(), 2);
        let json: Vec<u8> =
            s.db.query_row("SELECT payload FROM objects WHERE id=?", [&nid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Note>(&json).unwrap().text(),
            "second revision"
        );
        let asset = s.import_bytes(b"private-photo").unwrap();
        assert_eq!(s.asset(&asset).unwrap(), b"private-photo");
        assert_eq!(
            std::fs::read(root.join("assets").join(asset)).unwrap(),
            b"private-photo"
        );
        drop(s);
        let s = Store::open(root).unwrap();
        assert_eq!(s.notes.len(), 1);
        assert_eq!(s.notes[0].markdown(), "second revision");
    }

    #[test]
    fn stale_note_and_cross_kind_id_collision_cannot_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let mut note = Note::new();
        note.set_markdown("original".into());
        let stale = note.clone();
        store.save_note(note).unwrap();
        assert!(store.save_note(stale).is_err());
        let mut collision = Note::new();
        collision.id = DEFAULT_GROUP.into();
        collision.title = "collision".into();
        assert!(store.save_note(collision).is_err());
        assert_eq!(store.notes.len(), 1);
        assert_eq!(store.groups.len(), 1);
    }
}

#[cfg(test)]
mod group_style_tests {
    use super::*;
    #[test]
    fn directory_has_one_owner_until_store_is_dropped() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().to_path_buf()).unwrap();
        assert!(Store::open(temp.path().to_path_buf()).is_err());
        drop(store);
        assert!(Store::open(temp.path().to_path_buf()).is_ok());
    }
    #[test]
    fn old_groups_get_default_icon_and_styles_roundtrip() {
        let old: Group =
            serde_json::from_str(r#"{"id":"old","name":"旧分组","color":2380630}"#).unwrap();
        assert_eq!(old.icon, "dot");
        let root = std::env::temp_dir().join(format!("suojian-group-style-{}", id()));
        let mut store = Store::open(root.clone()).unwrap();
        let g = store
            .create_group_with_style("旅行", GROUP_COLORS[3], "image")
            .unwrap();
        let mut changed = g.clone();
        changed.color = GROUP_COLORS[7];
        changed.icon = "pin".into();
        store.save_group(changed).unwrap();
        drop(store);
        let store = Store::open(root).unwrap();
        let restored = store.groups.iter().find(|x| x.id == g.id).unwrap();
        assert_eq!(restored.icon, "pin");
        assert_eq!(restored.color, GROUP_COLORS[7]);
        assert_eq!(group_icon("unknown"), "dot");
    }
}
