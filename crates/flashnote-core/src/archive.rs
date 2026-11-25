//! Portable ZIP files contain domain data, never credentials or machine keys.
use crate::{
    model::*,
    store::{Store, write_object},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub groups: Vec<Group>,
    pub notes: Vec<Note>,
}
pub struct Import {
    pub manifest: Manifest,
    pub assets: BTreeMap<String, Vec<u8>>,
}
pub fn assets(notes: &[Note]) -> BTreeSet<String> {
    notes
        .iter()
        .flat_map(|n| n.blocks.iter().chain(&n.media_archive))
        .filter_map(|b| match b {
            Block::Image { asset, .. } | Block::Audio { asset, .. } | Block::File { asset, .. } => {
                Some(asset.clone())
            }
            _ => None,
        })
        .collect()
}
pub fn manifest(store: &Store, ids: &[String]) -> Manifest {
    let notes: Vec<_> = store
        .notes
        .iter()
        .filter(|n| ids.contains(&n.id) && !n.is_empty())
        .cloned()
        .collect();
    let groups = store
        .groups
        .iter()
        .filter(|g| {
            notes.iter().any(|n| n.group_id == g.id)
                || store
                    .notes
                    .iter()
                    .filter(|n| !n.is_empty() && n.deleted_at.is_none())
                    .all(|n| ids.contains(&n.id))
        })
        .cloned()
        .collect();
    Manifest {
        format: "suojian".into(),
        version: 1,
        groups,
        notes,
    }
}
pub fn export(root: PathBuf, manifest: Manifest, destination: &Path) -> Result<()> {
    let temporary = destination.with_extension(format!("{}.partial", id()));
    let result = (|| -> Result<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", options)?;
        zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        for asset in assets(&manifest.notes) {
            uuid::Uuid::parse_str(&asset)?;
            zip.start_file(format!("assets/{asset}"), options)?;
            let mut source = std::fs::File::open(root.join("assets").join(asset))?;
            std::io::copy(&mut source, &mut zip)?;
        }
        zip.finish()?.sync_all()?;
        // Same-directory rename replaces only after the complete ZIP is durable.
        std::fs::rename(&temporary, destination)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result
}
pub fn read(path: &Path) -> Result<Import> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    ensure!(zip.len() <= 20_001, "压缩包文件数量过多");
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        ensure!(names.insert(entry.name().to_owned()), "压缩包包含重复路径");
        ensure!(
            entry.enclosed_name().is_some() && !entry.is_symlink(),
            "压缩包包含无效路径"
        );
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("压缩包过大"))?;
        ensure!(
            total <= 512 * 1024 * 1024,
            "当前支持解压后不超过 512 MB 的压缩包"
        );
    }
    let mut json = Vec::new();
    zip.by_name("manifest.json")?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut json)?;
    ensure!(json.len() <= 16 * 1024 * 1024, "便签清单过大");
    let manifest: Manifest = serde_json::from_slice(&json)?;
    ensure!(
        manifest.format == "suojian" && manifest.version == 1,
        "不支持的闪记压缩包版本"
    );
    let mut ids = BTreeSet::new();
    for group in &manifest.groups {
        uuid::Uuid::parse_str(&group.id)?;
        ensure!(
            ids.insert(group.id.clone())
                && !group.name.trim().is_empty()
                && group.name.chars().count() <= 20
                && group.color <= 0xffffff,
            "分组数据无效或重复"
        );
    }
    for note in &manifest.notes {
        uuid::Uuid::parse_str(&note.id)?;
        ensure!(ids.insert(note.id.clone()), "便签 ID 重复");
        ensure!(
            note.group_id == DEFAULT_GROUP || manifest.groups.iter().any(|g| g.id == note.group_id),
            "便签分组缺失"
        );
    }
    let mut assets = BTreeMap::new();
    let mut remaining = 512 * 1024 * 1024u64;
    for asset in self::assets(&manifest.notes) {
        uuid::Uuid::parse_str(&asset)?;
        let mut bytes = Vec::new();
        zip.by_name(&format!("assets/{asset}"))?
            .take(remaining.min(crate::media::MAX_FILE) + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= remaining, "附件总大小过大");
        ensure!(
            bytes.len() as u64 <= crate::media::MAX_FILE,
            "附件超过 128 MiB"
        );
        remaining -= bytes.len() as u64;
        assets.insert(asset, bytes);
    }
    Ok(Import { manifest, assets })
}
impl Store {
    pub fn undo_batch(&mut self, before: &[Note], after: &[(String, String)]) -> Result<()> {
        ensure!(before.len() == after.len(), "撤销记录不完整");
        for (oid, revision) in after {
            ensure!(
                self.notes
                    .iter()
                    .any(|n| &n.id == oid && &n.revision == revision),
                "便签已再次修改或同步，不能撤销此次操作"
            );
        }
        let mut restored = Vec::new();
        let tx = self.db.transaction()?;
        for old in before {
            ensure!(
                self.groups.iter().any(|g| g.id == old.group_id),
                "原分组已不存在，不能撤销"
            );
            let mut note = old.clone();
            note.parent_revision = after
                .iter()
                .find(|(oid, _)| oid == &old.id)
                .map(|(_, revision)| revision.clone());
            note.revision = id();
            note.updated_at = now();
            write_object(
                &tx,
                &self.device,
                &note.id,
                "note",
                &note.revision,
                note.parent_revision.as_deref(),
                &note,
            )?;
            restored.push(note);
        }
        tx.commit()?;
        for note in restored {
            if let Some(old) = self.notes.iter_mut().find(|n| n.id == note.id) {
                *old = note;
            }
        }
        Ok(())
    }
    pub fn reorder_group(&mut self, gid: &str, direction: isize) -> Result<()> {
        let mut groups = self.groups.clone();
        let index = groups
            .iter()
            .position(|g| g.id == gid)
            .ok_or_else(|| anyhow::anyhow!("分组不存在"))?;
        let next = index as isize + direction;
        ensure!(
            index > 0 && next > 0 && next < groups.len() as isize,
            "无法继续移动"
        );
        groups.swap(index, next as usize);
        let tx = self.db.transaction()?;
        for (order, group) in groups.iter_mut().enumerate() {
            group.order = order as i64;
            write_object(&tx, &self.device, &group.id, "group", &id(), None, group)?;
        }
        tx.commit()?;
        self.groups = groups;
        Ok(())
    }
    pub fn delete_group(&mut self, gid: &str, destination: &str) -> Result<()> {
        ensure!(
            gid != DEFAULT_GROUP && gid != destination,
            "默认分组不能删除，迁移目标不能是原分组"
        );
        ensure!(
            self.groups.iter().any(|g| g.id == destination),
            "目标分组不存在"
        );
        let mut group = self
            .groups
            .iter()
            .find(|g| g.id == gid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("分组不存在"))?;
        group.deleted = true;
        let mut notes: Vec<_> = self
            .notes
            .iter()
            .filter(|n| n.group_id == gid)
            .cloned()
            .collect();
        let tx = self.db.transaction()?;
        for note in &mut notes {
            note.group_id = destination.into();
            note.parent_revision = Some(note.revision.clone());
            note.revision = id();
            note.updated_at = now();
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
        write_object(&tx, &self.device, gid, "group", &id(), None, &group)?;
        tx.commit()?;
        for note in notes {
            if let Some(old) = self.notes.iter_mut().find(|n| n.id == note.id) {
                *old = note;
            }
        }
        self.groups.retain(|g| g.id != gid);
        Ok(())
    }
    /// Keep an empty deletion revision for offline replicas; purge local history.
    pub fn purge_notes(&mut self, ids: &[String]) -> Result<()> {
        let mut notes: Vec<_> = self
            .notes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect();
        ensure!(
            notes.len() == ids.len() && notes.iter().all(|n| n.deleted_at.is_some()),
            "只能永久删除回收站中的便签"
        );
        let tx = self.db.transaction()?;
        for note in &mut notes {
            note.title.clear();
            note.blocks.clear();
            note.media_archive.clear();
            note.links.clear();
            note.media_depths.clear();
            note.conflict_of = None;
            note.conflict_resolved = true;
            note.parent_revision = Some(note.revision.clone());
            note.revision = id();
            note.updated_at = now();
            write_object(
                &tx,
                &self.device,
                &note.id,
                "note",
                &note.revision,
                note.parent_revision.as_deref(),
                note,
            )?;
            tx.execute("DELETE FROM sync_packets WHERE revision IN (SELECT revision FROM revisions WHERE object_id=? AND revision<>?)", rusqlite::params![note.id,note.revision])?;
            tx.execute("DELETE FROM outbox WHERE revision IN (SELECT revision FROM revisions WHERE object_id=? AND revision<>?)", rusqlite::params![note.id,note.revision])?;
            tx.execute(
                "DELETE FROM revisions WHERE object_id=? AND revision<>?",
                rusqlite::params![note.id, note.revision],
            )?;
        }
        tx.commit()?;
        for note in notes {
            if let Some(old) = self.notes.iter_mut().find(|n| n.id == note.id) {
                *old = note;
            }
        }
        Ok(())
    }
    pub fn cleanup_history(&mut self, days: u32) -> Result<usize> {
        ensure!(days > 0, "当前设置为保留全部历史");
        let cutoff = now() - i64::from(days) * 86_400_000;
        let tx = self.db.transaction()?;
        // Never remove a current revision, sync base, or unacknowledged upload.
        tx.execute("DELETE FROM outbox WHERE acknowledged=1 AND revision IN (SELECT revision FROM revisions WHERE created_at<?) AND revision NOT IN (SELECT revision FROM objects UNION SELECT revision FROM sync_heads)", [cutoff])?;
        tx.execute("DELETE FROM sync_packets WHERE revision IN (SELECT revision FROM revisions WHERE created_at<?) AND revision NOT IN (SELECT revision FROM objects UNION SELECT revision FROM sync_heads UNION SELECT revision FROM outbox)", [cutoff])?;
        let count = tx.execute("DELETE FROM revisions WHERE created_at<? AND revision NOT IN (SELECT revision FROM objects UNION SELECT revision FROM sync_heads UNION SELECT revision FROM outbox)", [cutoff])?;
        tx.commit()?;
        Ok(count)
    }
    pub fn cleanup_unused_assets(&self) -> Result<u64> {
        let mut keep = assets(&self.notes);
        let mut stmt = self.db.prepare("SELECT r.payload FROM revisions r JOIN objects o ON o.id=r.object_id WHERE o.kind='note' UNION ALL SELECT payload FROM sync_heads WHERE kind='note'")?;
        for row in stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            let note: Note = serde_json::from_slice(&row?)?;
            keep.extend(assets(&[note]));
        }
        let mut reclaimed = 0;
        for directory in ["assets", "sync-assets", "sync-image-receipts"] {
            for entry in std::fs::read_dir(self.root.join(directory))? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if uuid::Uuid::parse_str(&name).is_ok()
                    && !keep.contains(&name)
                    && entry.file_type()?.is_file()
                {
                    let bytes = entry.metadata()?.len();
                    std::fs::remove_file(entry.path())?;
                    reclaimed += bytes;
                }
            }
        }
        Ok(reclaimed)
    }
    pub fn import_archive(&mut self, import: &Import, keep_copies: bool) -> Result<usize> {
        let mut notes: Vec<_> = import
            .manifest
            .notes
            .iter()
            .filter(|n| {
                !n.is_empty() && (keep_copies || !self.notes.iter().any(|old| old.id == n.id))
            })
            .cloned()
            .collect();
        let mut groups = self.groups.clone();
        let mut group_map = HashMap::new();
        for incoming in &import.manifest.groups {
            if incoming.id == DEFAULT_GROUP {
                group_map.insert(incoming.id.clone(), DEFAULT_GROUP.to_string());
                continue;
            }
            if let Some(existing) = groups
                .iter()
                .find(|g| g.id == incoming.id || g.name.eq_ignore_ascii_case(&incoming.name))
            {
                group_map.insert(incoming.id.clone(), existing.id.clone());
            } else {
                let mut group = incoming.clone();
                group.id = id();
                group_map.insert(incoming.id.clone(), group.id.clone());
                groups.push(group);
            }
        }
        let mut asset_map = HashMap::new();
        let result = (|| -> Result<()> {
            for aid in assets(&notes) {
                let bytes = import
                    .assets
                    .get(&aid)
                    .ok_or_else(|| anyhow::anyhow!("附件缺失"))?;
                asset_map.insert(aid, self.import_bytes(bytes)?);
            }
            let mut note_map = HashMap::new();
            for note in &notes {
                note_map.insert(
                    note.id.clone(),
                    if self.notes.iter().any(|n| n.id == note.id) {
                        id()
                    } else {
                        note.id.clone()
                    },
                );
            }
            for note in &mut notes {
                note.id = note_map[&note.id].clone();
                note.revision = id();
                note.parent_revision = None;
                note.conflict_of = None;
                note.conflict_resolved = false;
                note.group_id = group_map
                    .get(&note.group_id)
                    .cloned()
                    .unwrap_or_else(|| DEFAULT_GROUP.into());
                for link in &mut note.links {
                    if let Some(mapped) = note_map.get(link) {
                        *link = mapped.clone();
                    }
                }
                for block in note.blocks.iter_mut().chain(&mut note.media_archive) {
                    match block {
                        Block::Image { asset, .. }
                        | Block::Audio { asset, .. }
                        | Block::File { asset, .. } => *asset = asset_map[asset].clone(),
                        _ => {}
                    }
                }
            }
            let tx = self.db.transaction()?;
            for group in groups
                .iter()
                .filter(|g| !self.groups.iter().any(|old| old.id == g.id))
            {
                write_object(&tx, &self.device, &group.id, "group", &id(), None, group)?;
            }
            for note in &notes {
                write_object(
                    &tx,
                    &self.device,
                    &note.id,
                    "note",
                    &note.revision,
                    None,
                    note,
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = result {
            for asset in asset_map.values() {
                let _ = std::fs::remove_file(self.root.join("assets").join(asset));
            }
            return Err(e);
        }
        let count = notes.len();
        self.groups = groups;
        self.sort_groups();
        self.notes.extend(notes);
        Ok(count)
    }
    pub fn batch_notes(
        &mut self,
        ids: &[String],
        group: Option<&str>,
        deleted: Option<bool>,
    ) -> Result<()> {
        if let Some(group) = group {
            ensure!(self.groups.iter().any(|g| g.id == group), "分组不存在");
        }
        let mut notes: Vec<_> = self
            .notes
            .iter()
            .filter(|n| ids.contains(&n.id) && !n.is_empty())
            .cloned()
            .collect();
        ensure!(notes.len() == ids.len(), "部分便签已变化，请重新选择");
        let tx = self.db.transaction()?;
        for note in &mut notes {
            if let Some(group) = group {
                note.group_id = group.into();
            }
            if let Some(deleted) = deleted {
                note.deleted_at = deleted.then(now);
            }
            note.parent_revision = Some(note.revision.clone());
            note.revision = id();
            note.updated_at = now();
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
        for note in notes {
            if let Some(old) = self.notes.iter_mut().find(|n| n.id == note.id) {
                *old = note;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(store: &mut Store) -> Note {
        let mut note = Note::new();
        note.set_markdown("压缩包往返测试".into());
        let asset = store.import_bytes(b"attachment bytes").unwrap();
        note.blocks.push(Block::File {
            id: id(),
            asset,
            name: "材料.txt".into(),
        });
        store.save_note(note.clone()).unwrap();
        store
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .unwrap()
            .clone()
    }
    #[test]
    fn failed_export_preserves_existing_file_and_undo_rejects_new_edits() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("source")).unwrap();
        let note = fixture(&mut store);
        let path = temp.path().join("existing.zip");
        std::fs::write(&path, b"previous backup").unwrap();
        assert!(
            export(
                temp.path().join("missing"),
                manifest(&store, &[note.id.clone()]),
                &path
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"previous backup");
        store
            .batch_notes(&[note.id.clone()], None, Some(true))
            .unwrap();
        let after = vec![(note.id.clone(), store.notes[0].revision.clone())];
        store.undo_batch(&[note.clone()], &after).unwrap();
        assert!(store.notes[0].deleted_at.is_none());
        assert!(store.undo_batch(&[note], &after).is_err());
    }
    #[test]
    fn zip_roundtrip_preserves_content_and_skip_or_copy_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let mut source = Store::open(temp.path().join("source")).unwrap();
        let note = fixture(&mut source);
        let path = temp.path().join("notes.zip");
        export(
            source.root.clone(),
            manifest(&source, &[note.id.clone()]),
            &path,
        )
        .unwrap();
        let imported = read(&path).unwrap();
        let mut destination = Store::open(temp.path().join("destination")).unwrap();
        assert_eq!(destination.import_archive(&imported, false).unwrap(), 1);
        assert_eq!(destination.import_archive(&imported, false).unwrap(), 0);
        assert_eq!(destination.import_archive(&imported, true).unwrap(), 1);
        assert_eq!(destination.notes.len(), 2);
        for note in &destination.notes {
            let aid = assets(&[note.clone()]).into_iter().next().unwrap();
            assert_eq!(destination.asset(&aid).unwrap(), b"attachment bytes");
        }
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        assert!(zip.by_name("vault-key.dpapi").is_err());
    }
    #[test]
    fn rejected_import_rolls_back_objects_and_new_assets() {
        let temp = tempfile::tempdir().unwrap();
        let mut source = Store::open(temp.path().join("source")).unwrap();
        let note = fixture(&mut source);
        let path = temp.path().join("notes.zip");
        export(source.root.clone(), manifest(&source, &[note.id]), &path).unwrap();
        let mut destination = Store::open(temp.path().join("destination")).unwrap();
        destination.db.execute_batch("CREATE TRIGGER reject_revision BEFORE INSERT ON revisions BEGIN SELECT RAISE(FAIL,'test failure'); END;").unwrap();
        assert!(
            destination
                .import_archive(&read(&path).unwrap(), false)
                .is_err()
        );
        assert!(destination.notes.is_empty());
        assert_eq!(
            std::fs::read_dir(destination.root.join("assets"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn invalid_zip_paths_and_missing_assets_are_rejected_before_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("../outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        assert!(read(&path).is_err());
        let mut store = Store::open(temp.path().join("source")).unwrap();
        let note = fixture(&mut store);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&manifest(&store, &[note.id])).unwrap())
            .unwrap();
        zip.finish().unwrap();
        assert!(read(&path).is_err());
    }
    #[test]
    fn group_migration_and_order_survive_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let a = store.create_group("甲").unwrap();
        let b = store.create_group("乙").unwrap();
        let mut note = fixture(&mut store);
        note.group_id = a.id.clone();
        store.save_note(note.clone()).unwrap();
        let index = store.groups.iter().position(|g| g.id == b.id).unwrap();
        if index > 1 {
            store.reorder_group(&b.id, -1).unwrap();
        }
        assert_eq!(store.groups[1].id, b.id);
        store.delete_group(&a.id, &b.id).unwrap();
        assert!(store.delete_group(DEFAULT_GROUP, &b.id).is_err());
        drop(store);
        let store = Store::open(temp.path().to_path_buf()).unwrap();
        assert_eq!(
            store
                .notes
                .iter()
                .find(|n| n.id == note.id)
                .unwrap()
                .group_id,
            b.id
        );
        assert!(!store.groups.iter().any(|g| g.id == a.id));
    }
    #[test]
    fn batch_failure_is_atomic_and_purge_retains_deletion_marker() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        let note = fixture(&mut store);
        let asset_id = assets(std::slice::from_ref(&note)).pop_first().unwrap();
        let cipher_cache = store.root.join("sync-assets").join(asset_id);
        std::fs::write(&cipher_cache, b"cached payload").unwrap();
        assert!(
            store
                .batch_notes(&[note.id.clone(), id()], None, Some(true))
                .is_err()
        );
        assert!(store.notes[0].deleted_at.is_none());
        assert!(store.purge_notes(&[note.id.clone()]).is_err());
        store
            .batch_notes(&[note.id.clone()], None, Some(true))
            .unwrap();
        assert_eq!(store.cleanup_unused_assets().unwrap(), 0);
        assert!(cipher_cache.exists());
        store.purge_notes(&[note.id.clone()]).unwrap();
        assert!(store.notes[0].is_empty() && store.notes[0].deleted_at.is_some());
        assert_eq!(store.history(&note.id).unwrap().len(), 1);
        assert!(store.cleanup_unused_assets().unwrap() > 0);
        assert!(!cipher_cache.exists());
        drop(store);
        let store = Store::open(temp.path().to_path_buf()).unwrap();
        assert!(store.notes[0].is_empty() && store.notes[0].deleted_at.is_some());
    }
}
