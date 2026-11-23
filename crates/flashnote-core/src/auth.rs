//! Casdoor 应用内账号密码认证、会话刷新和撤销。
use anyhow::{Context, Result, ensure};
use reqwest::{Url, blocking::Client};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Serialize, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub client_id: String,
}

#[derive(Deserialize)]
struct Tokens {
    issuer: String,
    client_id: String,
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub provider: Provider,
    pub token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub account_url: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
        self.refresh_token.zeroize();
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn secure_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )),
        "远程登录地址必须使用 HTTPS"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "登录地址不能包含凭据或片段"
    );
    Ok(url)
}

fn session(provider: Provider, tokens: Tokens) -> Result<Session> {
    ensure!(
        tokens.token_type.eq_ignore_ascii_case("bearer") && !tokens.access_token.is_empty(),
        "无效的登录令牌"
    );
    ensure!(
        (30..=86400).contains(&tokens.expires_in),
        "登录令牌有效期无效"
    );
    let refresh_token = tokens.refresh_token;
    ensure!(!refresh_token.is_empty(), "缺少刷新令牌");
    Ok(Session {
        provider,
        token: tokens.access_token,
        refresh_token,
        expires_at: now() + tokens.expires_in,
        account_url: String::new(),
    })
}

/// 应用内账号密码登录：不打开浏览器，密码仅用于本次 HTTPS 请求。
pub fn account_login(client: &Client, url: &str, username: &str, secret: &str) -> Result<Session> {
    secure_url(url)?;
    let response = client
        .post(format!("{}/auth/login", url.trim_end_matches('/')))
        .json(&serde_json::json!({"username":username,"secret":secret}))
        .send()?;
    ensure!(
        response.status() != reqwest::StatusCode::UNAUTHORIZED,
        "账号或密码错误，请重试"
    );
    let tokens: Tokens = serde_json::from_slice(&crate::sync::read_response(response, 8192)?)?;
    let mut value = session(
        Provider {
            issuer: tokens.issuer.clone(),
            client_id: tokens.client_id.clone(),
        },
        tokens,
    )?;
    value.account_url = url.trim_end_matches('/').into();
    Ok(value)
}

pub fn refresh(client: &Client, current: &Session) -> Result<Session> {
    let url = &current.account_url;
    secure_url(url)?;
    let response = client
        .post(format!("{url}/auth/refresh"))
        .json(&serde_json::json!({"refresh_token":current.refresh_token}))
        .send()?;
    let tokens: Tokens = serde_json::from_slice(
        &crate::sync::read_response(response, 8192).context("登录已过期，请重新登录")?,
    )
    .context("登录刷新接口返回的内容格式无效，请检查服务地址或代理配置")?;
    let mut value = session(current.provider.clone(), tokens)?;
    value.account_url = url.clone();
    Ok(value)
}

pub fn revoke(client: &Client, current: &Session) -> Result<()> {
    let url = &current.account_url;
    secure_url(url)?;
    crate::sync::read_response(
        client
            .post(format!("{url}/auth/logout"))
            .json(&serde_json::json!({"refresh_token":current.refresh_token}))
            .send()?,
        8192,
    )?;
    Ok(())
}
