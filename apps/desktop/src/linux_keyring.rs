use crate::crypto::{self, VaultKey};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use oo7::{Secret, dbus::Service};
use std::fs::{self, OpenOptions};
use zeroize::Zeroizing;

const ATTRIBUTES: [(&str, &str); 2] = [
    ("application", "NoteHarbor.Suojian"),
    ("purpose", "local-wrapping-key"),
];
const CONTEXT: &str = "suojian:linux-local-key:v1";

pub fn install() -> Result<()> {
    crypto::install_protector(protect)
}

fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    if decrypt {
        return futures_lite::future::block_on(wrapping_key(false))?.open(bytes, CONTEXT);
    }

    // Serialize first-time key creation across separate profile directories and the CLI.
    let legacy = directories::ProjectDirs::from("cn", "Suojian", "Suojian")
        .context("无法定位用户数据目录")?;
    let legacy_lock = legacy.data_local_dir().join("keyring.lock");
    let lock_path = if legacy_lock.is_file() {
        legacy_lock
    } else {
        crate::cli::default_data_dir().join("keyring.lock")
    };
    fs::create_dir_all(lock_path.parent().context("无法定位密钥环锁目录")?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    FileExt::lock_exclusive(&lock)?;
    futures_lite::future::block_on(wrapping_key(true))?.seal(bytes, CONTEXT)
}

async fn wrapping_key(create: bool) -> Result<VaultKey> {
    let service = Service::encrypted()
        .await
        .context("无法连接 Secret Service，请启动系统密钥环")?;
    let collection = if create {
        service.default_collection().await?
    } else {
        service
            .with_alias(Service::DEFAULT_COLLECTION)
            .await?
            .context("系统默认密钥环不存在")?
    };
    collection.unlock(None).await.context("请解锁系统密钥环")?;
    let attributes = ATTRIBUTES.to_vec();
    let items = collection.search_items(&attributes).await?;
    ensure!(items.len() <= 1, "系统密钥环中存在多个闪记密钥");
    if let Some(item) = items.first() {
        item.unlock(None).await.context("无法解锁闪记密钥")?;
        let secret = item.secret().await.context("无法读取闪记密钥")?;
        let mut key = Zeroizing::new([0; 32]);
        ensure!(secret.as_bytes().len() == key.len(), "闪记密钥格式无效");
        key.copy_from_slice(secret.as_bytes());
        return Ok(VaultKey(key));
    }

    ensure!(create, "系统密钥环中缺少闪记密钥，无法读取加密数据");
    let key = VaultKey::random();
    collection
        .create_item(
            "NoteHarbor local encryption key",
            &attributes,
            Secret::blob(&*key.0),
            false,
            None,
        )
        .await
        .context("无法保存闪记密钥")?;
    Ok(key)
}
