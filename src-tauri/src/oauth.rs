//! 百度 OAuth2.0 授权与 access_token 管理。两种模式（构建时决定，可用 BAIDU_AUTH_MODE 显式指定）：
//!  - 授权码 + 授权服务器（方案 C，分发用）：start_auth 返回百度授权页地址，
//!    用户浏览器登录（支持扫码）授权后，由授权服务器保管 SecretKey 换取 token，
//!    客户端轮询服务器取回。客户端只需内置 AppKey + 服务器地址，不含 SecretKey。
//!    若同时内置了 SecretKey，授权服务器不可达时自动回退设备码模式。
//!  - 设备码流程（方案 A）：获取 device_code / user_code / 二维码，用户浏览器
//!    输入用户码或扫码授权，后端轮询换取 token（需内置 AppKey/SecretKey）。
//! token 30 天有效，过期后用 refresh_token 自动续期（方案 C 下优先由服务器代理续期）。

use crate::config;
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const DEVICE_CODE_URL: &str = "https://openapi.baidu.com/oauth/2.0/device/code";
const AUTHORIZE_URL: &str = "https://openapi.baidu.com/oauth/2.0/authorize";
const TOKEN_URL: &str = "https://openapi.baidu.com/oauth/2.0/token";
const SCOPE: &str = "basic,netdisk";
/// 提前 5 分钟判定过期
const EXPIRE_MARGIN: u64 = 300;
/// 等待用户完成授权的最长时间（秒）
const AUTH_WINDOW_SECS: u64 = 300;
/// 授权码模式轮询服务器的间隔（秒）
const WEB_POLL_INTERVAL_SECS: u64 = 2;

#[derive(Serialize, Clone)]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_url: String,
    pub qrcode_url: String,
}

/// start_auth 的返回：网页授权（授权页地址）或设备码授权
#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum AuthStart {
    Web { url: String },
    Device(DeviceCodeInfo),
}

/// 当前进行中的授权会话（内存态）
#[derive(Clone)]
enum AuthSessionKind {
    Device {
        device_code: String,
        interval: u64,
    },
    Web {
        state: String,
    },
}

struct AuthSession {
    kind: AuthSessionKind,
    deadline: Instant,
}
static AUTH_SESSION: std::sync::Mutex<Option<AuthSession>> = std::sync::Mutex::new(None);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn save_session(kind: AuthSessionKind) -> Result<(), String> {
    let mut guard = AUTH_SESSION.lock().map_err(|e| e.to_string())?;
    *guard = Some(AuthSession {
        kind,
        deadline: Instant::now() + Duration::from_secs(AUTH_WINDOW_SECS),
    });
    Ok(())
}

/// 生成随机 state（两次随机种子哈希拼接），防止串号 / CSRF
fn gen_state() -> String {
    use std::collections::hash_map::RandomState;
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let mut s = String::with_capacity(32);
    for i in 0u8..2 {
        let mut h = RandomState::new().build_hasher();
        h.write(&nanos.to_le_bytes());
        h.write(&pid.to_le_bytes());
        h.write(&[i]);
        let _ = write!(s, "{:016x}", h.finish());
    }
    s
}

/// URL 百分号编码（OAuth redirect_uri 需要）
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 保存 token 到配置
fn store_tokens(
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
) -> Result<(), String> {
    config::update(|c| {
        c.access_token = Some(access_token);
        c.refresh_token = refresh_token;
        c.expires_at = Some(now() + expires_in);
    })
    .map(|_| ())
}

/// 是否启用网页授权（构建时决定）：BAIDU_AUTH_MODE 显式优先，否则有授权服务器地址即启用
pub fn web_mode_enabled() -> bool {
    match config::embedded_auth_mode() {
        config::AuthMode::Web => true,
        config::AuthMode::Device => false,
        config::AuthMode::Auto => config::embedded_server_url().is_some(),
    }
}

/// 发起授权：按构建配置选择 网页授权码模式（方案 C）或 设备码模式（方案 A）
pub async fn start_auth() -> Result<AuthStart, String> {
    if web_mode_enabled() {
        let cfg = config::current();
        if cfg.app_key.is_empty() {
            return Err(
                "当前构建未内置 AppKey，请设置环境变量 BAIDU_APP_KEY 后重新构建".into(),
            );
        }
        let server = config::embedded_server_url().ok_or(
            "已设置 BAIDU_AUTH_MODE=web，但未设置 BAIDU_AUTH_SERVER（授权服务器地址）",
        )?;
        let state = gen_state();
        let redirect_uri = format!("{server}/callback");
        let url = format!(
            "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&scope={SCOPE}&state={state}",
            cfg.app_key,
            percent_encode(&redirect_uri),
        );
        save_session(AuthSessionKind::Web { state })?;
        Ok(AuthStart::Web { url })
    } else {
        let (info, _) = request_device_code().await?;
        Ok(AuthStart::Device(info))
    }
}

/// 设备码模式：申请设备码（返回展示信息，并建立授权会话）
async fn request_device_code() -> Result<(DeviceCodeInfo, AuthSessionKind), String> {
    let cfg = config::current();
    if cfg.app_key.is_empty() {
        return Err(
            "当前构建未内置 AppKey，请设置环境变量 BAIDU_APP_KEY 后重新构建".into(),
        );
    }
    if cfg.secret_key.is_empty() {
        return Err(
            "设备码模式需要内置 SecretKey（BAIDU_SECRET_KEY）；或设置 BAIDU_AUTH_SERVER 走网页授权"
                .into(),
        );
    }
    let url = format!(
        "{DEVICE_CODE_URL}?client_id={}&response_type=device_code&scope={}",
        cfg.app_key, SCOPE
    );
    let v: Value = reqwest::get(&url)
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(format!("获取设备码失败: {err}"));
    }
    let info = DeviceCodeInfo {
        user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
        verification_url: v["verification_url"]
            .as_str()
            .unwrap_or("https://openapi.baidu.com/device")
            .to_string(),
        qrcode_url: v["qrcode_url"].as_str().unwrap_or_default().to_string(),
    };
    let kind = AuthSessionKind::Device {
        device_code: v["device_code"].as_str().unwrap_or_default().to_string(),
        interval: v["interval"].as_u64().unwrap_or(5),
    };
    save_session(kind.clone())?;
    Ok((info, kind))
}

/// 设备码模式：轮询百度换 token。Ok(true) 表示授权成功
async fn poll_device_once(device_code: &str) -> Result<bool, String> {
    let cfg = config::current();
    let params = [
        ("grant_type", "device_token"),
        ("code", device_code),
        ("client_id", cfg.app_key.as_str()),
        ("client_secret", cfg.secret_key.as_str()),
    ];
    let v: Value = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;

    if v.get("access_token").is_some() {
        let access_token = v["access_token"].as_str().unwrap_or_default().to_string();
        let refresh_token = v["refresh_token"].as_str().map(|s| s.to_string());
        let expires_in = v["expires_in"].as_u64().unwrap_or(2592000);
        store_tokens(access_token, refresh_token, expires_in)?;
        return Ok(true);
    }
    match v["error"].as_str().unwrap_or_default() {
        "authorization_pending" => Ok(false),
        "user_not_authorized" => Err("用户拒绝了授权".into()),
        "expired_token" => Err("授权码已过期，请重新发起授权".into()),
        other => Err(format!("授权失败: {other}")),
    }
}

/// 授权码模式单次轮询结果
enum PollOutcome {
    Done,
    Pending,
    /// 连不上授权服务器（网络故障，可回退设备码模式）
    ServerUnreachable(String),
}

/// 授权码模式：轮询授权服务器取回 token
async fn poll_web_once(state: &str) -> Result<PollOutcome, String> {
    let server = config::embedded_server_url().ok_or("未内置授权服务器地址")?;
    let url = format!("{server}/result?state={state}");
    let v: Value = match reqwest::get(&url).await {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                return Ok(PollOutcome::ServerUnreachable(format!("响应解析失败: {e}")));
            }
        },
        Err(e) => {
            return Ok(PollOutcome::ServerUnreachable(format!("网络请求失败: {e}")));
        }
    };
    match v["status"].as_str().unwrap_or_default() {
        "ok" => {
            let access_token = v["access_token"].as_str().unwrap_or_default().to_string();
            if access_token.is_empty() {
                return Err("授权服务器返回的 token 为空".into());
            }
            let refresh_token = v["refresh_token"].as_str().map(|s| s.to_string());
            let expires_in = v["expires_in"].as_u64().unwrap_or(2592000);
            store_tokens(access_token, refresh_token, expires_in)?;
            Ok(PollOutcome::Done)
        }
        "pending" => Ok(PollOutcome::Pending),
        "error" => Err(format!("授权失败: {}", v["message"].as_str().unwrap_or("未知错误"))),
        _ => Err("授权服务器响应异常".into()),
    }
}

fn clear_session() {
    if let Ok(mut s) = AUTH_SESSION.lock() {
        *s = None;
    }
}

/// 持续轮询直到成功 / 超时 / 失败。
/// 网页授权模式下若授权服务器不可达且内置了 SecretKey，自动回退设备码模式，
/// 并通过 auth://fallback 事件通知前端切换为扫码界面。
pub async fn wait_for_token(app: &AppHandle) -> Result<(), String> {
    loop {
        let (kind, deadline) = {
            let guard = AUTH_SESSION.lock().map_err(|e| e.to_string())?;
            let s = guard.as_ref().ok_or("没有进行中的授权会话")?;
            (s.kind.clone(), s.deadline)
        };
        match kind {
            AuthSessionKind::Device {
                device_code,
                interval,
            } => {
                if poll_device_once(&device_code).await? {
                    clear_session();
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err("等待授权超时，请重试".into());
                }
                tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
            }
            AuthSessionKind::Web { state } => {
                match poll_web_once(&state).await? {
                    PollOutcome::Done => {
                        clear_session();
                        return Ok(());
                    }
                    PollOutcome::Pending => {
                        if Instant::now() >= deadline {
                            return Err("等待授权超时，请重试".into());
                        }
                        tokio::time::sleep(Duration::from_secs(WEB_POLL_INTERVAL_SECS)).await;
                    }
                    PollOutcome::ServerUnreachable(msg) => {
                        // 授权服务器不可达：仅内置了 SecretKey 的构建可回退设备码扫码
                        if config::current().secret_key.trim().is_empty() {
                            return Err(format!("无法连接授权服务器: {msg}"));
                        }
                        let (info, _) = request_device_code().await?;
                        let _ = app.emit("auth://fallback", &info);
                        // 下一轮循环按新的设备码会话继续轮询
                    }
                }
            }
        }
    }
}

/// 通过授权服务器代理续期，成功返回新 token
async fn refresh_via_server(server: &str, rt: &str) -> Result<String, String> {
    let v: Value = reqwest::Client::new()
        .post(format!("{server}/refresh"))
        .json(&serde_json::json!({ "refresh_token": rt }))
        .send()
        .await
        .map_err(|e| format!("访问授权服务器失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    if v.get("access_token").is_none() {
        return Err(format!(
            "刷新失败: {}",
            v["message"].as_str().unwrap_or("未知错误")
        ));
    }
    let token = v["access_token"].as_str().unwrap_or_default().to_string();
    let refresh_token = v["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = v["expires_in"].as_u64().unwrap_or(2592000);
    store_tokens(token.clone(), refresh_token, expires_in)?;
    Ok(token)
}

/// 用 refresh_token 续期；网页授权模式优先由服务器代理（SecretKey 不落客户端），
/// 服务器不可用且内置了 SecretKey 时回退直连百度续期。
pub async fn refresh() -> Result<String, String> {
    let cfg = config::current();
    if cfg.app_key.is_empty() {
        return Err("当前构建未内置 AppKey，无法续期，请重新授权".into());
    }
    let rt = cfg
        .refresh_token
        .clone()
        .ok_or("缺少 refresh_token，请重新授权")?;

    if let Some(server) = config::embedded_server_url() {
        match refresh_via_server(&server, &rt).await {
            Ok(token) => return Ok(token),
            Err(msg) => {
                // 授权服务器不可用：仅内置了 SecretKey 的构建可直连百度续期
                if cfg.secret_key.trim().is_empty() {
                    return Err(msg);
                }
            }
        }
    }

    // 直连百度续期（方案 A 构建 / 服务器不可用时的回退）
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", rt.as_str()),
        ("client_id", cfg.app_key.as_str()),
        ("client_secret", cfg.secret_key.as_str()),
    ];
    let v: Value = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    if let Some(token) = v["access_token"].as_str() {
        let token = token.to_string();
        let refresh_token = v["refresh_token"].as_str().map(|s| s.to_string());
        let expires_in = v["expires_in"].as_u64().unwrap_or(2592000);
        store_tokens(token.clone(), refresh_token, expires_in)?;
        return Ok(token);
    }
    Err(format!(
        "刷新失败: {}",
        v["error_description"].as_str().unwrap_or("未知错误")
    ))
}

/// 获取可用的 access_token：不存在则报错，快过期则自动续期
pub async fn ensure_token() -> Result<String, String> {
    let cfg = config::current();
    let token = cfg.access_token.clone().ok_or("尚未登录，请先授权")?;
    let expired = cfg
        .expires_at
        .map(|at| now() + EXPIRE_MARGIN >= at)
        .unwrap_or(true);
    if !expired {
        return Ok(token);
    }
    refresh().await
}

#[derive(Serialize)]
pub struct Settings {
    pub logged_in: bool,
    /// 是否为网页授权码模式（分发版）
    pub web_auth: bool,
}
