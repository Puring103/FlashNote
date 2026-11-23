//! Local format v3: JSON records and original attachment bytes. Only transport
//! caches and credentials are encrypted.
use crate::{crypto::VaultKey, model::id};
use anyhow::{Result, ensure};
use std::{io::Write, path::Path};

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_file_name(format!("{}.tmp", id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// The same asset ID must always upload identical ciphertext, including retries
/// after a restart or a lost acknowledgement.
pub(crate) fn wire_asset(root: &Path, key: &VaultKey, aid: &str) -> Result<Vec<u8>> {
    uuid::Uuid::parse_str(aid)?;
    let plain = std::fs::read(root.join("assets").join(aid))?;
    let cache = root.join("sync-assets").join(aid);
    let aad = format!("suojian/asset/v1/{aid}");
    if cache.exists() {
        let wire = std::fs::read(cache)?;
        ensure!(
            key.open(&wire, &aad)? == plain,
            "附件内容已在应用之外修改，请重新导入为新附件"
        );
        return Ok(wire);
    }
    let wire = key.seal(&plain, &aad)?;
    atomic_write(&cache, &wire)?;
    Ok(wire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn cached_asset_retries_are_identical_and_immutable() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("vault")).unwrap();
        let aid = store.import_bytes(b"private bytes").unwrap();
        let wire = wire_asset(&store.root, &store.key, &aid).unwrap();
        assert_ne!(wire, b"private bytes");
        assert_eq!(
            store
                .key
                .open(&wire, &format!("suojian/asset/v1/{aid}"))
                .unwrap(),
            b"private bytes"
        );
        let root = store.root.clone();
        drop(store);
        let store = Store::open(root).unwrap();
        assert_eq!(wire_asset(&store.root, &store.key, &aid).unwrap(), wire);
        std::fs::write(store.root.join("assets").join(&aid), b"external change").unwrap();
        assert!(wire_asset(&store.root, &store.key, &aid).is_err());
    }
}
