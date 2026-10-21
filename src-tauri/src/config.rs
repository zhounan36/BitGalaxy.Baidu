//! 应用配置与 Token 持久化。
//! 配置文件保存在用户数据目录（Windows: %APPDATA%/baidupan-lite/config.json）。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AppConfig {
    pub app_key: String,
    pub secret_key: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// access_token 过期时间（unix 秒）
    pub expires_at: Option<u64>,
}

pub static CONFIG: Mutex<Option<AppConfig>> = Mutex::new(None);

/// 构建时通过环境变量 BAIDU_APP_KEY / BAIDU_SECRET_KEY 内置的密钥。
/// 分发版用户无需填写任何密钥，扫码登录自己的百度账号即可。
const EMBEDDED_APP_KEY: Option<&str> = option_env!("BAIDU_APP_KEY");
const EMBEDDED_SECRET_KEY: Option<&str> = option_env!("BAIDU_SECRET_KEY");

/// 返回内置密钥 (AppKey, SecretKey)；未内置时返回 None
pub fn embedded_keys() -> Option<(String, String)> {
    let ak = EMBEDDED_APP_KEY?.trim();
    let sk = EMBEDDED_SECRET_KEY.unwrap_or("").trim();
    if ak.is_empty() {
        None
    } else {
        Some((ak.to_string(), sk.to_string()))
    }
}

/// 构建时通过环境变量 BAIDU_AUTH_SERVER 内置的授权服务器地址（方案 C），
/// 例如 http://1.2.3.4:8787。设置后客户端走授权码模式，无需内置 SecretKey。
const EMBEDDED_AUTH_SERVER: Option<&str> = option_env!("BAIDU_AUTH_SERVER");

/// 返回内置的授权服务器地址（去除末尾 /）；未设置返回 None
pub fn embedded_server_url() -> Option<String> {
    EMBEDDED_AUTH_SERVER
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// 构建时通过 BAIDU_AUTH_MODE 显式指定授权模式：
/// web=仅网页授权（授权码 + 授权服务器），device=仅设备码扫码；
/// 不设置时自动判断（有授权服务器地址则网页授权，否则设备码）。
const EMBEDDED_AUTH_MODE: Option<&str> = option_env!("BAIDU_AUTH_MODE");

/// 授权模式
pub enum AuthMode {
    /// 有授权服务器地址则网页授权，否则设备码
    Auto,
    /// 强制网页授权（授权码 + 授权服务器）
    Web,
    /// 强制设备码扫码
    Device,
}

pub fn embedded_auth_mode() -> AuthMode {
    match EMBEDDED_AUTH_MODE.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "web" => AuthMode::Web,
        "device" => AuthMode::Device,
        _ => AuthMode::Auto,
    }
}

fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join("baidupan-lite")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| std::env::var("HOME").unwrap_or_else(|_| ".".into()));
        PathBuf::from(base).join(".config").join("baidupan-lite")
    }
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// 从磁盘加载配置到内存（启动时调用一次）
pub fn init_config() {
    let mut cfg = fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<AppConfig>(&s).ok())
        .unwrap_or_default();
    // 内置密钥优先；若与已存 AppKey 不同，旧 token 已失效，一并清空
    if let Some((ak, sk)) = embedded_keys() {
        if cfg.app_key != ak {
            cfg.access_token = None;
            cfg.refresh_token = None;
            cfg.expires_at = None;
            cfg.app_key = ak;
        }
        cfg.secret_key = sk;
    }
    if let Ok(mut guard) = CONFIG.lock() {
        *guard = Some(cfg);
    }
}

/// 读取当前配置（返回克隆）
pub fn current() -> AppConfig {
    CONFIG
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

/// 修改并落盘
pub fn update(mutator: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, String> {
    let mut guard = CONFIG.lock().map_err(|e| e.to_string())?;
    let mut cfg = guard.clone().unwrap_or_default();
    mutator(&mut cfg);
    fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    fs::write(config_path(), json).map_err(|e| e.to_string())?;
    *guard = Some(cfg.clone());
    Ok(cfg)
}
