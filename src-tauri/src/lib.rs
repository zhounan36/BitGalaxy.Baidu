//! 百度网盘 Lite —— 轻量客户端（Tauri 后端命令层）

mod api_server;
mod config;
mod download;
mod oauth;
mod pan_api;
mod share;
mod upload;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

#[tauri::command]
fn get_settings() -> oauth::Settings {
    let c = config::current();
    oauth::Settings {
        logged_in: c.access_token.is_some(),
        web_auth: oauth::web_mode_enabled(),
    }
}

#[tauri::command]
fn get_api_port() -> Option<u16> {
    api_server::port()
}

#[derive(Serialize)]
struct McpScriptInfo {
    path: String,
    /// false = 文件已存在且为最新；true = 本次自动写入
    created: bool,
}

/// 自动把内置的 MCP 服务器脚本写入用户目录（帮助弹窗第 1 步）
#[tauri::command]
fn setup_mcp_script() -> Result<McpScriptInfo, String> {
    let (path, created) = api_server::install_mcp_script()?;
    Ok(McpScriptInfo { path, created })
}

#[tauri::command]
async fn start_auth() -> Result<oauth::AuthStart, String> {
    oauth::start_auth().await
}

#[tauri::command]
async fn wait_auth(app: AppHandle) -> Result<(), String> {
    oauth::wait_for_token(&app).await
}

#[tauri::command]
fn logout() -> Result<(), String> {
    config::update(|c| {
        c.access_token = None;
        c.refresh_token = None;
        c.expires_at = None;
    })
    .map(|_| ())
}

#[derive(Serialize)]
struct UserState {
    user: Value,
    total: u64,
    used: u64,
}

#[tauri::command]
async fn get_user() -> Result<UserState, String> {
    let user = pan_api::uinfo().await?;
    let quota = pan_api::quota().await?;
    Ok(UserState {
        user,
        total: quota["total"].as_u64().unwrap_or(0),
        used: quota["used"].as_u64().unwrap_or(0),
    })
}

#[tauri::command]
async fn list_files(
    dir: String,
    order: String,
    desc: bool,
    start: u32,
    limit: u32,
) -> Result<Vec<pan_api::FileItem>, String> {
    pan_api::list_dir(&dir, &order, desc, start, limit).await
}

#[tauri::command]
async fn search_files(key: String, dir: String) -> Result<Vec<pan_api::FileItem>, String> {
    pan_api::search(&key, &dir).await
}

#[tauri::command]
async fn delete_files(paths: Vec<String>) -> Result<(), String> {
    pan_api::delete(&paths).await
}

#[tauri::command]
async fn rename_file(path: String, new_name: String) -> Result<(), String> {
    pan_api::rename(&path, &new_name).await
}

#[tauri::command]
async fn create_dir(path: String) -> Result<(), String> {
    pan_api::mkdir(&path).await
}

#[tauri::command]
async fn upload_files(
    app: AppHandle,
    local_paths: Vec<String>,
    remote_dir: String,
) -> Result<(), String> {
    upload::upload_files(app, local_paths, remote_dir).await
}

#[tauri::command]
async fn download_file(
    app: AppHandle,
    fs_id: u64,
    save_path: String,
    name: String,
) -> Result<(), String> {
    download::download_file(app, fs_id, save_path, name).await
}

/// 暂停下载任务（保留 .part，继续时断点续传）
#[tauri::command]
fn pause_download(id: String) -> bool {
    download::set_paused(&id, true)
}

/// 继续已暂停的下载任务
#[tauri::command]
fn resume_download(id: String) -> bool {
    download::set_paused(&id, false)
}

/// 打开下载任务的目标文件夹（传入文件路径时打开其所在目录）
#[tauri::command]
fn open_download_folder(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    let dir = if p.is_dir() {
        p
    } else {
        match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => p,
        }
    };
    tauri_plugin_opener::open_path(dir, None::<&str>).map_err(|e| format!("打开文件夹失败: {e}"))
}

#[tauri::command]
async fn list_files_recursive(dirs: Vec<String>) -> Result<Vec<pan_api::FileItem>, String> {
    pan_api::list_files_recursive(&dirs).await
}

#[tauri::command]
async fn download_zip(
    app: AppHandle,
    items: Vec<download::ZipItem>,
    save_path: String,
    task_name: String,
) -> Result<(), String> {
    let id = format!("z:{:x}", download::now_ms());
    download::download_zip(app, items, save_path, task_name, id).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            config::init_config();
            // 本地 HTTP API（供 AI 等本机程序调用，仅监听 127.0.0.1）
            api_server::start(app.handle().clone());
            // 启动时自动把最新 MCP 脚本写入用户目录（升级/重装后立即更新，无需手动配置）
            if let Err(e) = api_server::install_mcp_script() {
                eprintln!("[mcp] 更新用户目录脚本失败: {e}");
            }
            // 窗口标题跟随产品名
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title("百度网盘 Lite");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_api_port,
            setup_mcp_script,
            start_auth,
            wait_auth,
            logout,
            get_user,
            list_files,
            search_files,
            delete_files,
            rename_file,
            create_dir,
            upload_files,
            download_file,
            pause_download,
            resume_download,
            download_zip,
            open_download_folder,
            list_files_recursive,
            share::share_parse,
            share::share_transfer,
            share::read_image_base64
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
