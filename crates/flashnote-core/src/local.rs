//! Local format v3: JSON records and original attachment bytes. Only transport
//! caches and credentials are encrypted.
use crate::{crypto::VaultKey, model::id};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

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

// Only originating devices need a receipt linking their original to the smaller upload.
fn image_receipt(original: &[u8], copy: &[u8]) -> Vec<u8> {
    [
        Sha256::digest(original).as_slice(),
        Sha256::digest(copy).as_slice(),
    ]
    .concat()
}

pub(crate) fn verify_asset(root: &Path, aid: &str, original: &[u8], copy: &[u8]) -> Result<()> {
    if original != copy {
        let receipt = std::fs::read(root.join("sync-image-receipts").join(aid)).unwrap_or_default();
        ensure!(
            receipt == image_receipt(original, copy),
            "同一附件 ID 的内容不一致，请重新导入被修改的附件"
        );
    }
    Ok(())
}

/// Decode legacy raw assets and gzip assets using separate authenticated contexts.
pub fn open_asset(key: &VaultKey, aid: &str, wire: &[u8]) -> Result<Vec<u8>> {
    let limit = crate::media::MAX_FILE;
    ensure!(wire.len() as u64 <= limit + 41, "附件超过 128 MiB");
    if let Some(ciphertext) = wire.strip_prefix(&[2]) {
        let compressed = key.open(ciphertext, &format!("suojian/asset/gzip/v2/{aid}"))?;
        let mut decoder = flate2::bufread::GzDecoder::new(compressed.as_slice());
        let mut plain = Vec::new();
        decoder.by_ref().take(limit + 1).read_to_end(&mut plain)?;
        ensure!(
            plain.len() as u64 <= limit && decoder.into_inner().is_empty(),
            "附件解压失败或超过 128 MiB"
        );
        return Ok(plain);
    }
    key.open(wire, &format!("suojian/asset/v1/{aid}"))
}

/// The same asset ID must always upload identical ciphertext, including retries
/// after a restart or a lost acknowledgement.
pub(crate) fn wire_asset(
    root: &Path,
    key: &VaultKey,
    aid: &str,
    is_image: bool,
) -> Result<Vec<u8>> {
    uuid::Uuid::parse_str(aid)?;
    let plain = std::fs::read(root.join("assets").join(aid))?;
    let cache = root.join("sync-assets").join(aid);
    ensure!(
        plain.len() as u64 <= crate::media::MAX_FILE,
        "附件超过 128 MiB"
    );
    if cache.exists() {
        let wire = std::fs::read(cache)?;
        verify_asset(root, aid, &plain, &open_asset(key, aid, &wire)?)?;
        return Ok(wire);
    }
    let copy = if is_image {
        crate::media::sync_image(&plain)
    } else {
        None
    };
    if let Some(copy) = &copy {
        atomic_write(
            &root.join("sync-image-receipts").join(aid),
            &image_receipt(&plain, copy),
        )?;
    }
    let mut wire = None;
    if !is_image {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&plain)?;
        let compressed = encoder.finish()?;
        if compressed.len() + 1 < plain.len() {
            let mut tagged = vec![2];
            tagged.extend(key.seal(&compressed, &format!("suojian/asset/gzip/v2/{aid}"))?);
            wire = Some(tagged);
        }
    }
    let wire = match wire {
        Some(wire) => wire,
        None => key.seal(
            copy.as_deref().unwrap_or(&plain),
            &format!("suojian/asset/v1/{aid}"),
        )?,
    };
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
        let wire = wire_asset(&store.root, &store.key, &aid, false).unwrap();
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
        assert_eq!(
            wire_asset(&store.root, &store.key, &aid, false).unwrap(),
            wire
        );
        std::fs::write(store.root.join("assets").join(&aid), b"external change").unwrap();
        assert!(wire_asset(&store.root, &store.key, &aid, false).is_err());
    }
}
