//! Personal-device synchronization. The network worker never opens the local DB.
//! It transfers authenticated ciphertext; merging and cursor commits stay atomic
//! on the Store owner thread, so edits made during a request are not overwritten.
use crate::{
    auth,
    crypto::{VaultKey, protect},
    model::*,
    store::{Store, upsert_object},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};

static SESSION_LOCK: Mutex<()> = Mutex::new(());
const MAX_RESPONSE: u64 = 256 * 1024 * 1024;
const MAX_ASSET: u64 = 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub url: String,
    pub vault_id: String,
    pub issuer: String,
    pub subject: String,
    pub name: String,
    pub generation: String,
    #[serde(default)]
    pub e2ee: bool,
    pub session: auth::Session,
}

pub struct Connection {
    pub config: Config,
    pub key: VaultKey,
}

/// 服务端仅在验证账号归属后返回；调用方把主密钥写入本机 DPAPI。
#[derive(Deserialize)]
struct AccountKey {
    issuer: String,
    subject: String,
    name: String,
    vault_id: String,
    key: String,
}

#[derive(Deserialize)]
struct LoginConfig {
    #[serde(default)]
    vault_protocol: u32,
    mode: String,
}

fn login_config(c: &Client, url: &str) -> Result<LoginConfig> {
    let config: LoginConfig = serde_json::from_slice(&read_response(
        c.get(format!("{}/auth/config", url.trim_end_matches('/')))
            .send()?,
        8192,
    )?)?;
    ensure!(
        config.mode == "casdoor",
        "该地址未提供 Casdoor 登录，请检查同步服务器配置"
    );
    Ok(config)
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: u32,
    pub vault_id: String,
    pub object_id: String,
    pub kind: String,
    pub revision: String,
    pub base_revision: Option<String>,
    pub device: String,
    pub assets: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub header: Header,
    pub ciphertext: String,
}

#[derive(Clone)]
struct LocalObject {
    id: String,
    kind: String,
    revision: String,
    payload: Vec<u8>,
}

struct Upload {
    local: LocalObject,
    envelope: Envelope,
}

pub struct Plan {
    config: Config,
    key: VaultKey,
    root: PathBuf,
    cursor: u64,
    checkpoint: Option<String>,
    uploads: Vec<Upload>,
}

pub struct Cycle {
    reset: bool,
    upload_error: Option<String>,
    generation: String,
    uploaded: Vec<LocalObject>,
    incoming: Vec<Envelope>,
    after: u64,
    cursor: u64,
    more: bool,
    pub usage: Usage,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Usage {
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub asset_bytes: u64,
    pub asset_count: u64,
    pub object_bytes: u64,
    pub object_count: u64,
}

#[derive(Default, Serialize, Debug)]
pub struct Report {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_error: Option<String>,
    pub uploaded: usize,
    pub downloaded: usize,
    pub merged: usize,
    pub conflicts: usize,
    pub usage: Usage,
    #[serde(skip)]
    pub more: bool,
}

fn validate_server_address(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    let local = matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    ensure!(
        parsed.scheme() == "https" || (parsed.scheme() == "http" && local),
        "非本机同步地址必须使用 HTTPS"
    );
    ensure!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && matches!(parsed.path(), "" | "/"),
        "同步地址只能包含协议、主机与端口"
    );
    Ok(())
}

/// Accept an HTTPS origin or a host (with optional port), defaulting to HTTPS.
pub fn parse_server_address(address: &str) -> Result<String> {
    let address = address.trim();
    ensure!(!address.is_empty(), "请输入同步服务器网址或 IP");
    ensure!(
        !address.starts_with('/')
            && !address.contains(['\\', '@'])
            && !address.chars().any(char::is_whitespace),
        "同步地址只能包含协议、主机与端口"
    );
    let url = if address.contains("://") {
        address.to_owned()
    } else if address.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("https://[{address}]")
    } else {
        format!("https://{address}")
    };
    validate_server_address(&url)?;
    Ok(url.trim_end_matches('/').into())
}

pub(crate) fn client(url: &str) -> Result<Client> {
    validate_server_address(url)?;
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?)
}

pub(crate) fn read_response(response: Response, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        response.status().is_success(),
        "同步服务返回 HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "同步响应过大");
    Ok(bytes)
}

fn endpoint(config: &Config, route: &str) -> String {
    format!(
        "{}/v1/{}/{}",
        config.url.trim_end_matches('/'),
        config.vault_id,
        route
    )
}

fn context(header: &Header) -> Result<String> {
    Ok(format!(
        "suojian/sync/v1/{}",
        serde_json::to_string(header)?
    ))
}

fn assets(note: &Note) -> Vec<String> {
    note.blocks
        .iter()
        .chain(&note.media_archive)
        .filter_map(|b| match b {
            Block::Image { asset, .. } | Block::Audio { asset, .. } | Block::File { asset, .. }
                if !asset.is_empty() =>
            {
                Some(asset.clone())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn validate(envelope: &Envelope, config: &Config, key: &VaultKey) -> Result<Vec<u8>> {
    let h = &envelope.header;
    ensure!(
        h.version == 1 && h.vault_id == config.vault_id,
        "同步库或协议不匹配"
    );
    for value in [&h.object_id, &h.revision, &h.device] {
        uuid::Uuid::parse_str(value)?;
    }
    if let Some(base) = &h.base_revision {
        uuid::Uuid::parse_str(base)?;
    }
    for asset in &h.assets {
        uuid::Uuid::parse_str(asset)?;
    }
    let plain = key.open(&B64.decode(&envelope.ciphertext)?, &context(h)?)?;
    match h.kind.as_str() {
        "note" => {
            let n: Note =
                serde_json::from_slice(&plain).context("云端便签数据格式无效，未应用此版本")?;
            ensure!(
                n.id == h.object_id && n.revision == h.revision && assets(&n) == h.assets,
                "便签身份或附件引用不匹配"
            );
        }
        "group" => {
            let g: Group =
                serde_json::from_slice(&plain).context("云端分组数据格式无效，未应用此版本")?;
            ensure!(
                g.id == h.object_id && h.assets.is_empty() && g.color <= 0xffffff,
                "分组数据无效"
            );
            ensure!(
                g.id != DEFAULT_GROUP || g.name == "默认",
                "默认分组不能重命名"
            );
        }
        _ => bail!("不允许同步此对象类型"),
    }
    Ok(plain)
}

pub fn config(store: &Store) -> Result<Option<Config>> {
    let _guard = SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_config(&store.root)
}

fn read_config(root: &Path) -> Result<Option<Config>> {
    let path = root.join("sync-session.dpapi");
    if !path.exists() {
        return Ok(None);
    }
    let plain = zeroize::Zeroizing::new(protect(&std::fs::read(path)?, true)?);
    let value: Config = serde_json::from_slice(&plain)?;
    // Version 2 could have been created using the built-in service address.
    if value.version == 2 {
        return Ok(None);
    }
    ensure!(
        value.version == 3
            && !value.subject.is_empty()
            && value.issuer == value.session.provider.issuer,
        "同步登录信息无效，请重新登录"
    );
    ensure!(value.session.account_url == value.url, "登录服务地址不匹配");
    uuid::Uuid::parse_str(&value.vault_id)?;
    client(&value.url)?;
    Ok(Some(value))
}

fn write_config(root: &Path, value: &Config) -> Result<()> {
    let plain = zeroize::Zeroizing::new(serde_json::to_vec(value)?);
    crate::local::atomic_write(&root.join("sync-session.dpapi"), &protect(&plain, false)?)
}

/// 使用 Casdoor 会话领取该账号的同步密钥。
pub fn account_with_session(url: &str, session: auth::Session) -> Result<Connection> {
    let c = client(url)?;
    let response = c
        .post(format!("{}/v1/account/key", url.trim_end_matches('/')))
        .bearer_auth(&session.token)
        .json(&serde_json::json!({}))
        .send()?;
    let bytes = zeroize::Zeroizing::new(read_response(response, 8192)?);
    let mut account: AccountKey = serde_json::from_slice(&bytes)?;
    let encoded = zeroize::Zeroizing::new(std::mem::take(&mut account.key));
    ensure!(
        account.issuer == session.provider.issuer && !account.subject.is_empty(),
        "同步服务与身份服务不匹配"
    );
    uuid::Uuid::parse_str(&account.vault_id)?;
    let plain = zeroize::Zeroizing::new(B64.decode(encoded.as_bytes())?);
    ensure!(plain.len() == 32, "同步密钥长度无效");
    let mut key = VaultKey(zeroize::Zeroizing::new([0; 32]));
    key.0.copy_from_slice(&plain);
    Ok(Connection {
        config: Config {
            e2ee: false,
            version: 3,
            url: url.trim_end_matches('/').into(),
            vault_id: account.vault_id,
            issuer: account.issuer,
            subject: account.subject,
            name: account.name,
            generation: uuid::Uuid::new_v4().to_string(),
            session,
        },
        key,
    })
}

#[derive(Deserialize)]
struct VaultInfo {
    mode: String,
    vault_id: Option<String>,
    fingerprint: Option<String>,
    issuer: String,
    subject: String,
    name: String,
}

fn recovery_file(root: &Path, vault: &str) -> Result<PathBuf> {
    let id = uuid::Uuid::parse_str(vault)?;
    Ok(root.join(format!("recovery-{id}.protected")))
}

pub fn recovery_words(store: &Store) -> Result<zeroize::Zeroizing<String>> {
    let cfg = config(store)?
        .filter(|cfg| cfg.e2ee)
        .context("当前账号没有端到端加密助记词")?;
    let entropy = zeroize::Zeroizing::new(protect(
        &std::fs::read(recovery_file(&store.root, &cfg.vault_id)?)?,
        true,
    )?);
    Ok(zeroize::Zeroizing::new(
        bip39::Mnemonic::from_entropy(&entropy)?.to_string(),
    ))
}

fn open_vault(
    c: &Client,
    url: &str,
    session: auth::Session,
    root: &Path,
    words: &str,
    allow_managed: bool,
) -> Result<Connection> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    let request = |body: serde_json::Value| -> Result<VaultInfo> {
        let response = c
            .post(format!("{url}/v1/account/vault"))
            .bearer_auth(&session.token)
            .json(&body)
            .send()?;
        Ok(serde_json::from_slice(&read_response(response, 8192)?)?)
    };
    let mut info = request(serde_json::json!({}))?;
    // The identity provider can expose a user-facing name separately from the login ID.
    if let Ok(response) = c
        .get(format!("{url}/api/userinfo"))
        .bearer_auth(&session.token)
        .send()
        && let Ok(bytes) = read_response(response, 8192)
        && let Ok(profile) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && profile["sub"].as_str() == Some(info.subject.as_str())
        && let Some(name) = profile["name"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 512)
    {
        info.name = name.to_owned();
    }
    ensure!(
        info.issuer == session.provider.issuer && !info.subject.is_empty(),
        "身份服务不匹配"
    );
    if info.mode == "managed" {
        ensure!(
            allow_managed,
            "此资料库使用托管密钥，服务器能解密内容。请确认允许托管模式后重试"
        );
        ensure!(
            words.trim().is_empty(),
            "该资料库使用服务器托管密钥，无需输入助记词"
        );
        let mut connection = account_with_session(url, session)?;
        connection.config.name = info.name;
        return Ok(connection);
    }
    ensure!(info.mode == "e2ee", "未知的加密模式");
    let vault = info.vault_id.context("缺少资料库标识")?;
    let path = recovery_file(root, &vault)?;
    let owner = Owner {
        url: url.into(),
        issuer: info.issuer.clone(),
        subject: info.subject.clone(),
        vault_id: vault.clone(),
    };
    let binding = root.join("sync-owner.json");
    if binding.exists() {
        ensure!(
            serde_json::from_slice::<Owner>(&std::fs::read(binding)?)? == owner,
            "请为其他账户使用独立资料目录"
        );
    }
    let mut entropy = zeroize::Zeroizing::new([0u8; 16]);
    if !words.trim().is_empty() {
        ensure!(
            info.fingerprint.is_some(),
            "新资料库将自动生成助记词，请清空输入框后重试"
        );
        let phrase = bip39::Mnemonic::parse_in(bip39::Language::English, words.trim())
            .map_err(|_| anyhow::anyhow!("请输入正确的 12 个英文助记词"))?;
        ensure!(phrase.word_count() == 12, "需要 12 个助记词");
        entropy.copy_from_slice(&zeroize::Zeroizing::new(phrase.to_entropy()));
    } else if path.exists() {
        let saved = zeroize::Zeroizing::new(protect(&std::fs::read(&path)?, true)?);
        ensure!(saved.len() == 16, "本机助记词文件无效");
        entropy.copy_from_slice(&saved);
    } else {
        ensure!(
            info.fingerprint.is_none(),
            "新设备需要原有的 12 个助记词才能解密"
        );
        rand::rngs::OsRng.fill_bytes(&mut *entropy);
    }
    let mut key = VaultKey(zeroize::Zeroizing::new([0; 32]));
    hkdf::Hkdf::<Sha256>::new(Some(vault.as_bytes()), &*entropy)
        .expand(b"flashnote/e2ee/v1/key", &mut *key.0)
        .map_err(|_| anyhow::anyhow!("密钥派生失败"))?;
    let fingerprint = format!(
        "{:x}",
        Sha256::new()
            .chain_update(b"flashnote/e2ee/v1/check")
            .chain_update(&*key.0)
            .finalize()
    );
    if let Some(remote) = info.fingerprint {
        ensure!(remote == fingerprint, "助记词与该资料库不匹配");
    }
    // Secure the recovery material locally before the first network claim.
    crate::local::atomic_write(&path, &protect(&*entropy, false)?)?;
    let accepted = request(serde_json::json!({"vault_id": vault, "fingerprint": fingerprint}))?;
    ensure!(
        accepted.mode == "e2ee"
            && accepted.vault_id.as_ref() == Some(&vault)
            && accepted.fingerprint.as_ref() == Some(&fingerprint),
        "另一台设备已初始化资料库，请输入其助记词"
    );
    Ok(Connection {
        key,
        config: Config {
            version: 3,
            url: url.into(),
            vault_id: vault,
            issuer: info.issuer,
            subject: info.subject,
            name: info.name,
            generation: id(),
            e2ee: true,
            session,
        },
    })
}

pub fn login_device(
    root: &Path,
    words: &str,
    allow_managed: bool,
    url: &str,
    username: &str,
    secret: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<Connection> {
    let url = parse_server_address(url)?;
    let c = client(&url)?;
    let cfg = login_config(&c, &url)?;
    let session = auth::account_login(&c, &url, username, secret)?;
    let result = if cfg.vault_protocol == 0 {
        if allow_managed && words.trim().is_empty() {
            account_with_session(&url, session.clone())
        } else {
            Err(anyhow::anyhow!(
                "旧服务器仅支持托管密钥。请确认允许托管模式后重试"
            ))
        }
    } else {
        open_vault(&c, &url, session.clone(), root, words, allow_managed)
    };
    if result.is_err() || cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = auth::revoke(&c, &session);
        ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Relaxed),
            "登录已取消"
        );
    }
    result
}

pub fn login(
    url: &str,
    username: &str,
    secret: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<Connection> {
    let url = parse_server_address(url)?;
    let c = client(&url)?;
    ensure!(
        !cancelled.load(std::sync::atomic::Ordering::Relaxed),
        "登录已取消"
    );
    login_config(&c, &url)?;
    let session = auth::account_login(&c, &url, username, secret)?;
    finish_login(&url, &c, session, cancelled)
}

fn finish_login(
    url: &str,
    c: &Client,
    session: auth::Session,
    cancelled: Arc<AtomicBool>,
) -> Result<Connection> {
    let result = account_with_session(url, session.clone());
    if result.is_err() || cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = auth::revoke(&c, &session);
        ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Relaxed),
            "登录已取消"
        );
    }
    result
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Owner {
    url: String,
    issuer: String,
    subject: String,
    vault_id: String,
}

pub fn connect_account(store: &mut Store, connection: Connection) -> Result<()> {
    let value = &connection.config;
    let owner = Owner {
        url: value.url.clone(),
        issuer: value.issuer.clone(),
        subject: value.subject.clone(),
        vault_id: value.vault_id.clone(),
    };
    let owner_path = store.root.join("sync-owner.json");
    if owner_path.exists() {
        let previous: Owner = serde_json::from_slice(&std::fs::read(&owner_path)?)?;
        ensure!(
            previous == owner,
            "这个本地资料库已属于另一个账号或同步服务，请为其他账号使用独立资料目录"
        );
        ensure!(
            *store.key.0 == *connection.key.0,
            "账号密钥与本机资料库不一致"
        );
    } else {
        let tx = store.db.transaction()?;
        tx.execute_batch("DELETE FROM sync_heads; DELETE FROM sync_packets; DELETE FROM sync_state WHERE key='cursor'; UPDATE outbox SET acknowledged=0;")?;
        crate::local::atomic_write(
            &store.root.join("vault-key.dpapi"),
            &protect(&*connection.key.0, false)?,
        )?;
        tx.commit()?;
        store.key = connection.key;
        crate::local::atomic_write(&owner_path, &serde_json::to_vec(&owner)?)?;
    }
    let _guard = SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    write_config(&store.root, value)
}

/// Remove local authentication immediately; the caller revokes the refresh token
/// on a worker. Ownership and local notes stay bound to this account.
pub fn logout(store: &Store) -> Result<Option<Config>> {
    let _guard = SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = read_config(&store.root).ok().flatten();
    let path = store.root.join("sync-session.dpapi");
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(current)
}

pub fn revoke_session(value: &Config) -> Result<()> {
    auth::revoke(&client(&value.url)?, &value.session)
}

fn refresh_config(root: &Path, value: &mut Config) -> Result<()> {
    if value.session.expires_at > auth::now() + 60 {
        return Ok(());
    }
    let refreshed = auth::refresh(&client(&value.url)?, &value.session)?;
    // Persist token rotation even if the later data transfer fails. A late
    // worker cannot resurrect credentials after logout or a new login.
    let _guard = SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !read_config(root)
        .ok()
        .flatten()
        .is_some_and(|current| current.generation == value.generation)
    {
        drop(_guard);
        let _ = auth::revoke(&client(&value.url)?, &refreshed);
        bail!("登录已退出或更换，请重新同步");
    }
    value.session = refreshed;
    write_config(root, value)
}

pub fn pending_count(store: &Store) -> Result<i64> {
    Ok(store.db.query_row(
        "SELECT COUNT(*) FROM objects o LEFT JOIN sync_heads h ON h.id=o.id
        WHERE o.kind IN ('note','group') AND (h.revision IS NULL OR h.revision<>o.revision)",
        [],
        |row| row.get(0),
    )?)
}

pub fn prepare(store: &mut Store) -> Result<Option<Plan>> {
    prepare_cycle(store, true)
}

pub fn prepare_pull(store: &mut Store) -> Result<Option<Plan>> {
    prepare_cycle(store, false)
}

fn read_cursor(db: &rusqlite::Connection) -> Result<u64> {
    let value = db
        .query_row(
            "SELECT value FROM sync_state WHERE key='cursor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value.as_deref().unwrap_or("0").parse()?)
}

fn prepare_cycle(store: &mut Store, upload: bool) -> Result<Option<Plan>> {
    let Some(config) = config(store)? else {
        return Ok(None);
    };
    let cursor = read_cursor(&store.db)?;
    let rows = if upload {
        store
            .db
            .prepare(
                "SELECT o.id,o.kind,o.revision,o.payload,h.revision,p.packet FROM objects o
                 LEFT JOIN sync_heads h ON h.id=o.id
                 LEFT JOIN sync_packets p ON p.revision=o.revision
                 WHERE o.kind IN ('note','group')
                   AND (h.revision IS NULL OR h.revision<>o.revision)
                 ORDER BY o.kind,o.id",
            )?
            .query_map([], |r| {
                Ok((
                    LocalObject {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        revision: r.get(2)?,
                        payload: r.get(3)?,
                    },
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
    } else {
        Vec::new()
    };
    let mut uploads = vec![];
    for (local, base, cached) in rows {
        let envelope = if let Some(packet) = cached {
            let packet: Envelope = serde_json::from_str(&packet)?;
            ensure!(
                packet.header.base_revision == base,
                "待同步版本的基础版本发生变化"
            );
            packet
        } else {
            let refs = if local.kind == "note" {
                let note: Note = serde_json::from_slice(&local.payload)?;
                if note.blocks.iter().any(|b| matches!(b, Block::Audio{asset,..} | Block::Image{asset,..} | Block::File{asset,..} if asset.is_empty())) {
                    continue; // Recording/import placeholders are never sent to another device.
                }
                assets(&note)
            } else {
                vec![]
            };
            let header = Header {
                version: 1,
                vault_id: config.vault_id.clone(),
                object_id: local.id.clone(),
                kind: local.kind.clone(),
                revision: local.revision.clone(),
                base_revision: base,
                device: store.device.clone(),
                assets: refs,
            };
            let packet = Envelope {
                ciphertext: B64.encode(store.key.seal(&local.payload, &context(&header)?)?),
                header,
            };
            store.db.execute(
                "INSERT INTO sync_packets VALUES(?,?)",
                params![local.revision, serde_json::to_string(&packet)?],
            )?;
            packet
        };
        uploads.push(Upload { local, envelope });
    }
    Ok(Some(Plan {
        config,
        key: VaultKey(zeroize::Zeroizing::new(*store.key.0)),
        root: store.root.clone(),
        checkpoint: store
            .db
            .query_row(
                "SELECT value FROM sync_state WHERE key='checkpoint'",
                [],
                |r| r.get(0),
            )
            .optional()?,
        cursor,
        uploads,
    }))
}

fn download_assets(c: &Client, plan: &Plan, packet: &Envelope) -> Result<()> {
    for aid in &packet.header.assets {
        let path = plan.root.join("assets").join(aid);
        let cache = plan.root.join("sync-assets").join(aid);
        let bytes = if cache.exists() {
            std::fs::read(&cache)?
        } else {
            read_response(
                c.get(endpoint(&plan.config, &format!("assets/{aid}")))
                    .bearer_auth(&plan.config.session.token)
                    .send()?,
                MAX_ASSET,
            )?
        };
        let plain = plan.key.open(&bytes, &format!("suojian/asset/v1/{aid}"))?;
        if path.exists() {
            ensure!(std::fs::read(&path)? == plain, "同一附件 ID 的内容不一致");
        } else {
            crate::local::atomic_write(&path, &plain)?;
        }
        if !cache.exists() {
            crate::local::atomic_write(&cache, &bytes)?;
        }
    }
    Ok(())
}

pub fn transfer(mut plan: Plan) -> Result<Cycle> {
    refresh_config(&plan.root, &mut plan.config)?;
    let c = client(&plan.config.url)?;
    let reset = if plan.cursor == 0 {
        false
    } else if let Some(revision) = &plan.checkpoint {
        uuid::Uuid::parse_str(revision)?;
        #[derive(Deserialize)]
        struct Checkpoint {
            valid: bool,
        }
        let bytes = read_response(
            c.get(endpoint(
                &plan.config,
                &format!("checkpoint?seq={}&revision={revision}", plan.cursor),
            ))
            .bearer_auth(&plan.config.session.token)
            .send()?,
            4096,
        )?;
        !serde_json::from_slice::<Checkpoint>(&bytes)?.valid
    } else {
        true
    };
    let mut uploaded = vec![];
    let mut upload_error = None;
    for upload in plan.uploads.iter().filter(|_| !reset) {
        let attempt = (|| -> Result<bool> {
            for aid in &upload.envelope.header.assets {
                let bytes = crate::local::wire_asset(&plan.root, &plan.key, aid)?;
                ensure!(bytes.len() as u64 <= MAX_ASSET, "附件超过同步限制 64MiB");
                let url = endpoint(&plan.config, &format!("assets/{aid}"));
                let status = c
                    .head(&url)
                    .bearer_auth(&plan.config.session.token)
                    .send()?
                    .status();
                if status == StatusCode::NOT_FOUND {
                    read_response(
                        c.put(url)
                            .bearer_auth(&plan.config.session.token)
                            .body(bytes)
                            .send()?,
                        4096,
                    )?;
                } else {
                    ensure!(status.is_success(), "附件检查返回 HTTP {}", status.as_u16());
                }
            }
            let response = c
                .put(endpoint(
                    &plan.config,
                    &format!("objects/{}", upload.local.id),
                ))
                .bearer_auth(&plan.config.session.token)
                .json(&upload.envelope)
                .send()?;
            if response.status() == StatusCode::CONFLICT {
                // Only stale bases are mergeable. Immutable revision/kind violations
                // also use HTTP 409 and must surface as errors, without advancing state.
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct StaleBase {
                    current: Option<Envelope>,
                }
                let mut bytes = Vec::new();
                response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes)?;
                ensure!(bytes.len() as u64 <= MAX_RESPONSE, "同步响应过大");
                let stale: StaleBase = serde_json::from_slice(&bytes)
                    .context("同步服务返回 HTTP 409，且不是可合并的版本过期响应")?;
                let current = stale.current.context("云端基础版本丢失，无法安全合并")?;
                validate(&current, &plan.config, &plan.key)?;
                ensure!(
                    current.header.object_id == upload.local.id
                        && current.header.kind == upload.local.kind
                        && Some(&current.header.revision)
                            != upload.envelope.header.base_revision.as_ref(),
                    "云端版本过期响应与上传对象不匹配"
                );
                // Pull below obtains the newest server version. Keep this local edit pending.
                return Ok(false);
            }
            read_response(response, 4096)?;
            Ok(true)
        })();
        match attempt {
            Ok(true) => uploaded.push(upload.local.clone()),
            Ok(false) => (),
            Err(error) if !error.to_string().contains("HTTP 409") => {
                upload_error.get_or_insert_with(|| format!("{}：{error}", upload.local.id));
            }
            Err(error) => return Err(error),
        }
    }
    #[derive(Deserialize)]
    struct Page {
        items: Vec<Envelope>,
        cursor: u64,
        more: bool,
    }
    let mut cursor = if reset { 0 } else { plan.cursor };
    let mut incoming = vec![];
    // Apply each page before fetching the next one to bound pull memory.
    let more = {
        let page: Page = serde_json::from_slice(&read_response(
            c.get(endpoint(&plan.config, &format!("changes?after={cursor}")))
                .bearer_auth(&plan.config.session.token)
                .send()?,
            MAX_RESPONSE,
        )?)
        .context("同步服务返回的变更列表格式无效，请检查服务地址或代理配置")?;
        ensure!(
            page.cursor >= cursor && (!page.more || page.cursor > cursor),
            "同步游标无效"
        );
        for packet in page.items {
            validate(&packet, &plan.config, &plan.key)?;
            download_assets(&c, &plan, &packet)?;
            incoming.push(packet);
        }
        cursor = page.cursor;
        page.more
    };
    let usage = serde_json::from_slice(&read_response(
        c.get(endpoint(&plan.config, "usage"))
            .bearer_auth(&plan.config.session.token)
            .send()?,
        4096,
    )?)
    .context("同步服务返回的空间用量格式无效，请检查服务地址或代理配置")?;
    Ok(Cycle {
        reset,
        upload_error,
        generation: plan.config.generation.clone(),
        uploaded,
        incoming,
        after: plan.cursor,
        cursor,
        more,
        usage,
    })
}

fn head(tx: &Transaction<'_>, object: &LocalObject) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO sync_heads VALUES(?,?,?,?)",
        params![object.id, object.kind, object.revision, object.payload],
    )?;
    Ok(())
}

fn acknowledge(tx: &Transaction<'_>, object: &LocalObject) -> Result<()> {
    tx.execute(
        "UPDATE outbox SET acknowledged=1 WHERE revision IN (SELECT revision FROM revisions
        WHERE object_id=?1 AND rowid<=(SELECT rowid FROM revisions WHERE revision=?2))",
        params![object.id, object.revision],
    )?;
    tx.execute("DELETE FROM sync_packets WHERE revision IN (SELECT revision FROM outbox WHERE acknowledged=1)", [])?;
    Ok(())
}

fn install(
    tx: &Transaction<'_>,
    object: &LocalObject,
    parent: Option<&str>,
    device: &str,
    pending: bool,
) -> Result<()> {
    upsert_object(
        tx,
        &object.id,
        &object.kind,
        &object.revision,
        &object.payload,
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO revisions VALUES(?,?,?,?,?,?)",
        params![
            object.revision,
            object.id,
            parent,
            device,
            now(),
            object.payload
        ],
    )?;
    if pending {
        tx.execute(
            "INSERT OR IGNORE INTO outbox(revision) VALUES(?)",
            [&object.revision],
        )?;
    }
    Ok(())
}

fn choose<T: Clone + PartialEq>(base: &T, local: &T, remote: &T) -> Option<T> {
    if local == remote || remote == base {
        Some(local.clone())
    } else if local == base {
        Some(remote.clone())
    } else {
        None
    }
}

fn semantic(note: &Note) -> serde_json::Value {
    let mut value = serde_json::to_value(note).expect("Note is serializable");
    // Text block IDs are serialization details, unlike media node identities.
    for block in value["blocks"].as_array_mut().unwrap() {
        if block["kind"] == "markdown" {
            block.as_object_mut().unwrap().remove("id");
        }
    }
    for field in ["revision", "parent_revision", "updated_at", "created_at"] {
        value.as_object_mut().unwrap().remove(field);
    }
    value
}

fn merge_note(base: &Note, local: &Note, remote: &Note) -> Option<Note> {
    // An edit versus a deletion must keep a visible copy of the edited note.
    if (local.deleted_at != base.deleted_at
        && local.deleted_at.is_some()
        && semantic(remote) != semantic(base))
        || (remote.deleted_at != base.deleted_at
            && remote.deleted_at.is_some()
            && semantic(local) != semantic(base))
    {
        return None;
    }
    let mut merged = remote.clone();
    merged.title = choose(&base.title, &local.title, &remote.title)?;
    merged.group_id = choose(&base.group_id, &local.group_id, &remote.group_id)?;
    merged.links = choose(&base.links, &local.links, &remote.links)?;
    merged.deleted_at = choose(&base.deleted_at, &local.deleted_at, &remote.deleted_at)?;
    merged.conflict_of = choose(&base.conflict_of, &local.conflict_of, &remote.conflict_of)?;
    merged.conflict_resolved = choose(
        &base.conflict_resolved,
        &local.conflict_resolved,
        &remote.conflict_resolved,
    )?;
    let text = diffy::merge(
        &base.editor_markdown(),
        &local.editor_markdown(),
        &remote.editor_markdown(),
    )
    .ok()?;
    fn media(n: &Note) -> BTreeMap<String, Block> {
        n.media_archive
            .iter()
            .chain(&n.blocks)
            .filter(|b| !matches!(b, Block::Markdown { .. }))
            .map(|b| (b.id().into(), b.clone()))
            .collect()
    }
    let b = media(base);
    let l = media(local);
    let r = media(remote);
    let ids: BTreeSet<_> = b.keys().chain(l.keys()).chain(r.keys()).cloned().collect();
    let mut available = vec![];
    for bid in ids {
        if let Some(block) = choose(&b.get(&bid), &l.get(&bid), &r.get(&bid))? {
            available.push(block.clone());
        }
    }
    merged.blocks.clear();
    merged.media_archive = available;
    for (bid, depth) in &local.media_depths {
        merged.media_depths.entry(bid.clone()).or_insert(*depth);
    }
    merged.set_editor_markdown(&text);
    merged.parent_revision = Some(remote.revision.clone());
    merged.revision = id();
    merged.updated_at = now();
    Some(merged)
}

fn encode_local(
    kind: &str,
    oid: &str,
    revision: &str,
    data: &impl Serialize,
) -> Result<LocalObject> {
    Ok(LocalObject {
        id: oid.into(),
        kind: kind.into(),
        revision: revision.into(),
        payload: serde_json::to_vec(data)?,
    })
}

pub fn apply(store: &mut Store, cycle: Cycle) -> Result<Report> {
    let cfg = config(store)?
        .filter(|current| current.generation == cycle.generation)
        .context("登录已退出或更换，本轮同步结果已忽略")?;
    let mut report = Report {
        upload_error: cycle.upload_error,
        uploaded: cycle.uploaded.len(),
        usage: cycle.usage,
        more: cycle.more,
        ..Report::default()
    };
    let tx = store.db.transaction()?;
    ensure!(
        read_cursor(&tx)? == cycle.after,
        "同步进度已更新，请重新同步"
    );
    if cycle.reset {
        tx.execute_batch("DELETE FROM sync_heads; DELETE FROM sync_packets; DELETE FROM sync_state WHERE key='checkpoint';")?;
    }
    if let Some(last) = cycle.incoming.last() {
        tx.execute(
            "INSERT OR REPLACE INTO sync_state VALUES('checkpoint',?)",
            [&last.header.revision],
        )?;
    }
    let mut changed = BTreeSet::new();
    for local in &cycle.uploaded {
        head(&tx, local)?;
        acknowledge(&tx, local)?;
    }
    for packet in cycle.incoming {
        let plain = validate(&packet, &cfg, &store.key)?;
        let h = &packet.header;
        let base: Option<LocalObject> = tx
            .query_row(
                "SELECT id,kind,revision,payload FROM sync_heads WHERE id=?",
                [&h.object_id],
                |r| {
                    Ok(LocalObject {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        revision: r.get(2)?,
                        payload: r.get(3)?,
                    })
                },
            )
            .optional()?;
        if base.as_ref().is_some_and(|b| b.revision == h.revision) {
            continue;
        }
        let local: Option<LocalObject> = tx
            .query_row(
                "SELECT id,kind,revision,payload FROM objects WHERE id=?",
                [&h.object_id],
                |r| {
                    Ok(LocalObject {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        revision: r.get(2)?,
                        payload: r.get(3)?,
                    })
                },
            )
            .optional()?;
        ensure!(
            local.as_ref().is_none_or(|l| l.kind == h.kind),
            "对象类型不匹配"
        );
        let remote = LocalObject {
            id: h.object_id.clone(),
            kind: h.kind.clone(),
            revision: h.revision.clone(),
            payload: plain.clone(),
        };
        let dirty = local.as_ref().is_some_and(|l| {
            l.revision != h.revision && base.as_ref().is_none_or(|b| b.revision != l.revision)
        });
        let mut pending: Option<LocalObject> = None;
        if dirty {
            let l = local.as_ref().unwrap();
            let lp = l.payload.clone();
            let bp = base.as_ref().map(|b| b.payload.clone());
            if h.kind == "note" {
                let ln: Note = serde_json::from_slice(&lp)?;
                let rn: Note = serde_json::from_slice(&plain)?;
                let bn: Option<Note> = bp.as_deref().map(serde_json::from_slice).transpose()?;
                if semantic(&ln) != semantic(&rn) {
                    if let Some(merged) = bn.as_ref().and_then(|b| merge_note(b, &ln, &rn)) {
                        pending =
                            Some(encode_local("note", &merged.id, &merged.revision, &merged)?);
                        report.merged += 1;
                    } else {
                        let mut copy = ln;
                        let name =
                            format!("suojian/conflict/{}/{}/{}", l.id, l.revision, h.revision);
                        copy.id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes())
                            .to_string();
                        copy.revision = uuid::Uuid::new_v5(
                            &uuid::Uuid::NAMESPACE_OID,
                            format!("{name}/revision").as_bytes(),
                        )
                        .to_string();
                        copy.parent_revision = None;
                        copy.deleted_at = None;
                        copy.conflict_of = Some(h.object_id.clone());
                        copy.conflict_resolved = false;
                        let fork = encode_local("note", &copy.id, &copy.revision, &copy)?;
                        install(&tx, &fork, None, &store.device, true)?;
                        changed.insert(fork.id);
                        report.conflicts += 1;
                    }
                }
            } else {
                let lg: Group = serde_json::from_slice(&lp)?;
                let rg: Group = serde_json::from_slice(&plain)?;
                let bg: Option<Group> = bp.as_deref().map(serde_json::from_slice).transpose()?;
                if lg != rg {
                    let merged = bg.as_ref().and_then(|b| {
                        Some(Group {
                            order: choose(&b.order, &lg.order, &rg.order)?,
                            deleted: choose(&b.deleted, &lg.deleted, &rg.deleted)?,
                            id: rg.id.clone(),
                            name: choose(&b.name, &lg.name, &rg.name)?,
                            color: choose(&b.color, &lg.color, &rg.color)?,
                            icon: choose(&b.icon, &lg.icon, &rg.icon)?,
                        })
                    });
                    if let Some(g) = merged {
                        pending = Some(encode_local("group", &g.id, &id(), &g)?);
                        report.merged += 1;
                    } else if rg.id != DEFAULT_GROUP {
                        let name = format!(
                            "suojian/group-conflict/{}/{}/{}",
                            l.id, l.revision, h.revision
                        );
                        let mut g = lg;
                        g.id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes())
                            .to_string();
                        g.name = format!("{}（离线副本）", g.name);
                        let fork = encode_local("group", &g.id, &id(), &g)?;
                        install(&tx, &fork, None, &store.device, true)?;
                        changed.insert(fork.id);
                        report.conflicts += 1;
                    } else {
                        // The reserved default group cannot be duplicated. Keep local
                        // styling as a new update based on the downloaded revision.
                        pending = Some(encode_local("group", &lg.id, &id(), &lg)?);
                        report.merged += 1;
                    }
                }
            }
            acknowledge(&tx, l)?;
        }
        install(&tx, &remote, h.base_revision.as_deref(), &h.device, false)?;
        head(&tx, &remote)?;
        if let Some(pending) = pending {
            install(&tx, &pending, Some(&remote.revision), &store.device, true)?;
        }
        report.downloaded += 1;
        changed.insert(remote.id);
    }
    // A deleted group can arrive before or after a note created offline.
    let orphaned = {
        let mut stmt = tx.prepare("SELECT n.payload FROM objects n JOIN objects g ON g.id=json_extract(n.payload,'$.group_id') WHERE n.kind='note' AND g.kind='group' AND json_extract(g.payload,'$.deleted')=1")?;
        stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for bytes in orphaned {
        let mut note: Note = serde_json::from_slice(&bytes)?;
        let previous = note.revision.clone();
        note.group_id = DEFAULT_GROUP.into();
        note.parent_revision = Some(previous.clone());
        note.revision = id();
        let updated = encode_local("note", &note.id, &note.revision, &note)?;
        install(&tx, &updated, Some(&previous), &store.device, true)?;
        changed.insert(note.id);
        report.merged += 1;
    }
    tx.execute(
        "INSERT OR REPLACE INTO sync_state VALUES('cursor',?)",
        [cycle.cursor.to_string()],
    )?;
    // Deserialize before commit so a failed decode cannot leave the cache
    // behind the persisted cursor.
    let mut notes = vec![];
    let mut groups = vec![];
    {
        let mut statement = tx.prepare("SELECT kind,payload FROM objects WHERE id=?")?;
        for oid in &changed {
            let (kind, plain): (String, Vec<u8>) =
                statement.query_row([oid], |row| Ok((row.get(0)?, row.get(1)?)))?;
            if kind == "note" {
                notes.push(serde_json::from_slice(&plain)?);
            } else {
                groups.push(serde_json::from_slice(&plain)?);
            }
        }
    }
    tx.commit()?;
    if !notes.is_empty() {
        store.notes.retain(|n| !changed.contains(&n.id));
        store.notes.extend(notes);
    }
    if !groups.is_empty() {
        store.groups.retain(|g| !changed.contains(&g.id));
        store.groups.extend(groups);
        store.sort_groups();
    }
    Ok(report)
}

pub fn once(store: &mut Store) -> Result<Report> {
    let plan = prepare(store)?.context("尚未配置同步")?;
    let mut report = apply(store, transfer(plan)?)?;
    while report.more {
        let plan = prepare_pull(store)?.context("尚未配置同步")?;
        let page = apply(store, transfer(plan)?)?;
        report.downloaded += page.downloaded;
        report.merged += page.merged;
        report.conflicts += page.conflicts;
        report.usage = page.usage;
        report.more = page.more;
    }
    if let Some(error) = &report.upload_error {
        bail!("已接收云端变更，上传失败：{error}");
    }
    Ok(report)
}

pub fn usage(store: &Store) -> Result<Usage> {
    let mut cfg = config(store)?.context("尚未登录同步账号")?;
    refresh_config(&store.root, &mut cfg)?;
    Ok(serde_json::from_slice(&read_response(
        client(&cfg.url)?
            .get(endpoint(&cfg, "usage"))
            .bearer_auth(&cfg.session.token)
            .send()?,
        4096,
    )?)?)
}

#[cfg(test)]
mod semantic_tests {
    use super::*;
    #[test]
    fn text_identity_churn_does_not_turn_delete_into_conflict() {
        let mut base = Note::new();
        base.set_editor_markdown("正文");
        let mut unchanged = base.clone();
        if let Block::Markdown { id: bid, .. } = &mut unchanged.blocks[0] {
            *bid = id();
        }
        assert_eq!(semantic(&base), semantic(&unchanged));
        let mut deleted = base.clone();
        deleted.deleted_at = Some(now());
        assert!(
            merge_note(&base, &unchanged, &deleted)
                .unwrap()
                .deleted_at
                .is_some()
        );
        unchanged.set_editor_markdown("修改过的正文");
        assert!(merge_note(&base, &unchanged, &deleted).is_none());
    }
}
