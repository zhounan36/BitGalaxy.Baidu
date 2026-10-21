//! 本地 HTTP API：供 AI 等本机程序调用百度网盘（仅监听 127.0.0.1，token 不离开应用）。
//! 默认端口 8790，可用环境变量 BAIDU_API_PORT 指定；被占用时依次 +1 重试。
//!
//! 端点一览（JSON 返回，错误格式 {"error": "..."}）：
//!   GET  /                                              接口自描述
//!   GET  /health                                        应用与登录状态
//!   GET  /user                                          用户信息与网盘容量
//!   GET  /files?dir=/&order=name&desc=0&start=0&limit=100   列目录
//!   GET  /search?key=关键词&dir=/                        递归搜索
//!   POST /download        {fs_id, save_path, name?}      下载单个文件（异步任务）
//!   POST /download_dir    {dir, save_path}               整目录递归打包 zip（异步任务）
//!   POST /share/parse     {link, pwd?}                   解析分享链接（自动识别链接内提取码）
//!   POST /share/transfer  {shareid, uk, sekey?, fs_ids}  分享文件转存到自己网盘
//!   POST /share/save      {link, pwd?, save_path?}       一站式：解析→转存→下载（异步任务）
//!   GET  /tasks                                          下载任务进度
//!   GET  /mcp.js                                         下载 MCP 服务器脚本（配置进 AI 客户端用）
//!
//! 配套 server/mcp.js 把这些接口包装为 MCP 工具供 AI 客户端使用。

use crate::{config, download, oauth, pan_api, share};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Listener};

static PORT: OnceLock<u16> = OnceLock::new();

/// 内置于应用内的 MCP 服务器脚本（server/mcp.js），通过 GET /mcp.js 下发
const MCP_SCRIPT: &str = include_str!("../../server/mcp.js");

/// 当前本地 API 实际监听端口（未启动时为 None）
pub fn port() -> Option<u16> {
    PORT.get().copied()
}

/// 确保 MCP 服务器脚本存在于用户目录（<用户目录>/baidupan-mcp.js）：
/// 缺失或内容非最新时写入，返回 (路径, 是否本次写入)。
pub fn install_mcp_script() -> Result<(String, bool), String> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "无法获取用户目录".to_string())?;
    let path = std::path::Path::new(&home).join("baidupan-mcp.js");
    let up_to_date = std::fs::read_to_string(&path)
        .map(|c| c == MCP_SCRIPT)
        .unwrap_or(false);
    if !up_to_date {
        std::fs::write(&path, MCP_SCRIPT).map_err(|e| format!("写入脚本失败: {e}"))?;
    }
    Ok((path.to_string_lossy().into_owned(), !up_to_date))
}

/// 启动本地 API 服务器（独立线程，阻塞处理请求）
pub fn start(app: AppHandle) {
    // 监听下载进度事件，维护任务表供 /tasks 查询
    app.listen("transfer://progress", |e| {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload()) else {
            return;
        };
        if v["kind"].as_str() != Some("download") {
            return;
        }
        upsert_task(TaskEntry {
            id: v["id"].as_str().unwrap_or_default().to_string(),
            name: v["name"].as_str().unwrap_or_default().to_string(),
            total: v["total"].as_u64().unwrap_or(0),
            done: v["done"].as_u64().unwrap_or(0),
            phase: v["phase"].as_str().unwrap_or("unknown").to_string(),
            message: v["message"].as_str().unwrap_or_default().to_string(),
        });
    });

    std::thread::spawn(move || {
        let base: u16 = std::env::var("BAIDU_API_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8790);
        let mut bound = None;
        for port in base..base.saturating_add(10) {
            if let Ok(s) = tiny_http::Server::http(("127.0.0.1", port)) {
                bound = Some((s, port));
                break;
            }
        }
        let Some((server, port)) = bound else {
            eprintln!("[api] 端口 {base}~{} 均被占用，本地 API 未启动", base + 9);
            return;
        };
        let _ = PORT.set(port);
        eprintln!("[api] 本地 API 已启动: http://127.0.0.1:{port}（仅本机可访问）");
        for request in server.incoming_requests() {
            handle(&app, request);
        }
    });
}

// ---------- 任务表 ----------

#[derive(Serialize, Clone)]
struct TaskEntry {
    id: String,
    name: String,
    total: u64,
    done: u64,
    /// starting / downloading / packing / done / error
    phase: String,
    message: String,
}

static TASKS: Mutex<Vec<TaskEntry>> = Mutex::new(Vec::new());

fn upsert_task(t: TaskEntry) {
    if t.id.is_empty() {
        return;
    }
    if let Ok(mut v) = TASKS.lock() {
        match v.iter_mut().find(|x| x.id == t.id) {
            Some(slot) => *slot = t,
            None => v.push(t),
        }
        // 上限 50 条：优先淘汰已完成的旧任务
        if v.len() > 50 {
            let mut i = 0;
            while v.len() > 50 && i < v.len() {
                if v[i].phase == "done" || v[i].phase == "error" {
                    v.remove(i);
                } else {
                    i += 1;
                }
            }
        }
    }
}

// ---------- HTTP 处理 ----------

fn header(k: &str, v: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("非法响应头")
}

fn json_resp(code: u16, v: &Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(serde_json::to_string(v).unwrap_or_default())
        .with_status_code(code)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Access-Control-Allow-Origin", "*"))
}

fn handle(app: &AppHandle, mut req: tiny_http::Request) {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let query = parse_query(&url);

    if method == tiny_http::Method::Options {
        let resp = tiny_http::Response::empty(204)
            .with_header(header("Access-Control-Allow-Origin", "*"))
            .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
            .with_header(header("Access-Control-Allow-Headers", "Content-Type"));
        let _ = req.respond(resp);
        return;
    }

    if method == tiny_http::Method::Get && path == "/mcp.js" {
        let resp = tiny_http::Response::from_string(MCP_SCRIPT)
            .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
            .with_header(header("Access-Control-Allow-Origin", "*"));
        let _ = req.respond(resp);
        return;
    }

    let mut body = String::new();
    if method == tiny_http::Method::Post {
        let _ = req.as_reader().read_to_string(&mut body);
    }

    let result = route(app, &method, &path, &query, &body);
    let resp = match result {
        Ok(v) => json_resp(200, &v),
        Err((code, msg)) => json_resp(code, &json!({ "error": msg })),
    };
    let _ = req.respond(resp);
}

fn route(
    app: &AppHandle,
    method: &tiny_http::Method,
    path: &str,
    query: &HashMap<String, String>,
    body: &str,
) -> Result<Value, (u16, String)> {
    let is_get = *method == tiny_http::Method::Get;
    let is_post = *method == tiny_http::Method::Post;

    match path {
        "/" if is_get => Ok(json!({
            "app": "baidupan-lite",
            "api": "v1",
            "base": format!("http://127.0.0.1:{}", PORT.get().copied().unwrap_or(8790)),
            "note": "仅本机可访问；POST 接口请求体为 JSON",
            "endpoints": {
                "GET /health": "应用与登录状态",
                "GET /user": "用户信息与网盘容量",
                "GET /files?dir=&order=&desc=&start=&limit=": "列目录",
                "GET /search?key=&dir=": "递归搜索文件",
                "POST /download {fs_id, save_path, name?}": "下载单个文件（异步）",
                "POST /download_dir {dir, save_path}": "整目录打包 zip 下载（异步）",
                "POST /share/parse {link, pwd?}": "解析分享链接，返回文件列表（链接内提取码自动识别）",
                "POST /share/transfer {shareid, uk, sekey?, fs_ids}": "分享文件转存到自己网盘",
                "POST /share/save {link, pwd?, save_path?}": "一站式：解析分享→转存→下载到本地（异步）",
                "GET /tasks": "下载任务进度（phase: starting/downloading/packing/done/error）",
                "GET /mcp.js": "下载 MCP 服务器脚本（配置到 AI 客户端后，AI 即可使用上述能力）",
            },
        })),

        "/health" if is_get => Ok(json!({
            "app": "baidupan-lite",
            "logged_in": config::current().access_token.is_some(),
        })),

        "/user" if is_get => tauri::async_runtime::block_on(async {
            let user = pan_api::uinfo().await?;
            let quota = pan_api::quota().await?;
            Ok(json!({
                "user": user,
                "total": quota["total"].as_u64().unwrap_or(0),
                "used": quota["used"].as_u64().unwrap_or(0),
            }))
        })
        .map_err(auth_err),

        "/files" if is_get => {
            let dir = query.get("dir").cloned().unwrap_or_else(|| "/".into());
            let order = query.get("order").cloned().unwrap_or_else(|| "name".into());
            let desc = matches!(query.get("desc").map(String::as_str), Some("1") | Some("true"));
            let start = query.get("start").and_then(|s| s.parse().ok()).unwrap_or(0);
            let limit = query.get("limit").and_then(|s| s.parse().ok()).unwrap_or(100).min(1000);
            tauri::async_runtime::block_on(pan_api::list_dir(&dir, &order, desc, start, limit))
                .map(|list| serde_json::to_value(list).unwrap_or(Value::Null))
                .map_err(auth_err)
        }

        "/search" if is_get => {
            let key = query
                .get("key")
                .filter(|s| !s.is_empty())
                .ok_or((400u16, "缺少参数 key（搜索关键词）".to_string()))?;
            let dir = query.get("dir").cloned().unwrap_or_else(|| "/".into());
            tauri::async_runtime::block_on(pan_api::search(key, &dir))
                .map(|list| serde_json::to_value(list).unwrap_or(Value::Null))
                .map_err(auth_err)
        }

        "/download" if is_post => {
            let v: Value =
                serde_json::from_str(body).map_err(|e| (400u16, format!("请求体不是合法 JSON: {e}")))?;
            let fs_id = v["fs_id"]
                .as_u64()
                .ok_or((400u16, "缺少 fs_id（数字，可从 /files 或 /search 获取）".to_string()))?;
            let save_path = v["save_path"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or((400u16, "缺少 save_path（本地完整保存路径，含文件名）".to_string()))?
                .to_string();
            let name = v["name"]
                .as_str()
                .map(String::from)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    save_path
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("file")
                        .to_string()
                });
            tauri::async_runtime::block_on(oauth::ensure_token()).map_err(auth_err)?;
            let id = format!("d:{fs_id}");
            upsert_task(TaskEntry {
                id: id.clone(),
                name: name.clone(),
                total: 0,
                done: 0,
                phase: "starting".into(),
                message: String::new(),
            });
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = download::download_file(app, fs_id, save_path, name).await {
                    upsert_task(TaskEntry {
                        id: format!("d:{fs_id}"),
                        name: String::new(),
                        total: 0,
                        done: 0,
                        phase: "error".into(),
                        message: e,
                    });
                }
            });
            Ok(json!({ "id": id, "hint": "异步任务已开始，GET /tasks 查询进度" }))
        }

        "/download_dir" if is_post => {
            let v: Value =
                serde_json::from_str(body).map_err(|e| (400u16, format!("请求体不是合法 JSON: {e}")))?;
            let dir = v["dir"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or((400u16, "缺少 dir（网盘目录路径）".to_string()))?
                .to_string();
            let save_path = v["save_path"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or((400u16, "缺少 save_path（本地 zip 完整保存路径）".to_string()))?
                .to_string();
            tauri::async_runtime::block_on(oauth::ensure_token()).map_err(auth_err)?;
            let id = format!("z:{:x}", download::now_ms());
            let reply_id = id.clone();
            let task_name = format!(
                "目录打包：{}",
                dir.trim_end_matches('/').rsplit('/').next().unwrap_or(&dir)
            );
            upsert_task(TaskEntry {
                id: id.clone(),
                name: task_name.clone(),
                total: 0,
                done: 0,
                phase: "starting".into(),
                message: String::new(),
            });
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let files = match pan_api::list_files_recursive(&[dir.clone()]).await {
                    Ok(f) => f,
                    Err(e) => {
                        upsert_task(TaskEntry {
                            id,
                            name: task_name,
                            total: 0,
                            done: 0,
                            phase: "error".into(),
                            message: e,
                        });
                        return;
                    }
                };
                if files.is_empty() {
                    upsert_task(TaskEntry {
                        id,
                        name: task_name,
                        total: 0,
                        done: 0,
                        phase: "error".into(),
                        message: "目录中没有文件".into(),
                    });
                    return;
                }
                let prefix = dir.trim_end_matches('/');
                let items: Vec<download::ZipItem> = files
                    .iter()
                    .map(|f| download::ZipItem {
                        fs_id: f.fs_id,
                        rel_path: f
                            .path
                            .strip_prefix(prefix)
                            .unwrap_or(&f.path)
                            .trim_start_matches('/')
                            .to_string(),
                        size: f.size,
                    })
                    .collect();
                // download_zip 失败时会自行推送 error 事件并更新任务表
                let _ = download::download_zip(app, items, save_path, task_name, id).await;
            });
            Ok(json!({ "id": reply_id, "hint": "异步任务已开始，GET /tasks 查询进度" }))
        }

        // ---------- 分享链接：解析 / 转存 / 一站式转存下载 ----------

        "/share/parse" if is_post => {
            let v: Value = parse_json_body(body)?;
            let link = req_str(&v, "link", "缺少 link（分享链接，可整段粘贴含提取码的文本）")?;
            let pwd = v["pwd"].as_str().unwrap_or_default().to_string();
            let info = tauri::async_runtime::block_on(share::share_parse(link, pwd))
                .map_err(auth_err)?;
            let mut map = serde_json::to_value(&info).unwrap_or(Value::Null);
            if info.need_pwd {
                map["hint"] = json!("该分享需要提取码：请向用户询问 4 位提取码后带 pwd 参数重试");
            }
            Ok(map)
        }

        "/share/transfer" if is_post => {
            let v: Value = parse_json_body(body)?;
            let shareid = req_str(&v, "shareid", "缺少 shareid（来自 /share/parse 结果）")?;
            let uk = req_str(&v, "uk", "缺少 uk（来自 /share/parse 结果）")?;
            let sekey = v["sekey"].as_str().unwrap_or_default().to_string();
            let fs_ids: Vec<u64> = v["fs_ids"]
                .as_array()
                .map(|a| a.iter().filter_map(|i| i.as_u64()).collect())
                .unwrap_or_default();
            if fs_ids.is_empty() {
                return Err((400u16, "缺少 fs_ids（要转存的文件 id 数组，来自 /share/parse 结果）".into()));
            }
            tauri::async_runtime::block_on(oauth::ensure_token()).map_err(auth_err)?;
            let results =
                tauri::async_runtime::block_on(share::share_transfer(shareid, uk, sekey, fs_ids))
                    .map_err(auth_err)?;
            Ok(json!({
                "transferred": results,
                "hint": "已转存到自己网盘「来自：分享」目录，可用 GET /files 查看，或 POST /download 下载",
            }))
        }

        // 一站式：解析（自动填提取码）→ 转存 → 下载到本地目录。
        // save_path 为本地目录：文件直接存入；目录类分享打包为 <目录名>.zip。
        "/share/save" if is_post => {
            let v: Value = parse_json_body(body)?;
            let link = req_str(&v, "link", "缺少 link（分享链接，可整段粘贴含提取码的文本）")?;
            let pwd = v["pwd"].as_str().unwrap_or_default().to_string();
            let save_path = v["save_path"].as_str().unwrap_or_default().trim().to_string();
            let download = v["download"].as_bool().unwrap_or(true) && !save_path.is_empty();

            let info = tauri::async_runtime::block_on(share::share_parse(link, pwd))
                .map_err(auth_err)?;
            if info.need_pwd {
                return Ok(json!({
                    "need_pwd": true,
                    "hint": "该分享需要提取码：请向用户询问 4 位提取码后带 pwd 参数重试",
                }));
            }
            let fs_ids: Vec<u64> = info.files.iter().map(|f| f.fs_id).collect();
            if fs_ids.is_empty() {
                return Err((400u16, "分享内容为空".into()));
            }
            tauri::async_runtime::block_on(oauth::ensure_token()).map_err(auth_err)?;
            let shareid = info.shareid.clone();
            let uk = info.uk.clone();
            let sekey = info.sekey.clone();
            let results = tauri::async_runtime::block_on(share::share_transfer(
                shareid,
                uk,
                sekey,
                fs_ids,
            ))
            .map_err(auth_err)?;

            // 按需下载：文件 → 单文件任务；目录 → 打包 zip 任务
            let mut tasks = Vec::new();
            if download {
                let save_dir = save_path.trim_end_matches(['/', '\\']).to_string();
                for r in &results {
                    let Some(src) = info.files.iter().find(|f| f.fs_id == r.from_fs_id) else {
                        continue;
                    };
                    let (id, task_name) = if src.is_dir {
                        (format!("z:{:x}", r.fs_id), format!("目录打包：{}", src.name))
                    } else {
                        (format!("d:{}", r.fs_id), src.name.clone())
                    };
                    upsert_task(TaskEntry {
                        id: id.clone(),
                        name: task_name.clone(),
                        total: 0,
                        done: 0,
                        phase: "starting".into(),
                        message: String::new(),
                    });
                    let app = app.clone();
                    let save_dir = save_dir.clone();
                    let id2 = id.clone();
                    let name = src.name.clone();
                    let is_dir = src.is_dir;
                    let netdisk_path = r.path.clone();
                    let fs_id = r.fs_id;
                    tauri::async_runtime::spawn(async move {
                        let res = if is_dir {
                            download_dir_zip(&app, &netdisk_path, &save_dir, &name, id2.clone())
                                .await
                        } else {
                            let dest = format!("{}\\{}", save_dir, name);
                            download::download_file(app, fs_id, dest, name).await
                        };
                        if let Err(e) = res {
                            upsert_task(TaskEntry {
                                id: id2,
                                name: String::new(),
                                total: 0,
                                done: 0,
                                phase: "error".into(),
                                message: e,
                            });
                        }
                    });
                    tasks.push(json!({ "id": id, "name": task_name }));
                }
            }

            Ok(json!({
                "need_pwd": false,
                "transferred": results,
                "download": if download {
                    json!({
                        "save_dir": save_path,
                        "tasks": tasks,
                        "hint": "下载任务已开始，GET /tasks 查询进度，phase=done 即完成",
                    })
                } else {
                    Value::Null
                },
                "hint": if download {
                    "已转存并开始下载".to_string()
                } else {
                    "已转存到自己网盘「来自：分享」目录（未提供 save_path，跳过下载）".to_string()
                },
            }))
        }

        "/tasks" if is_get => {
            let tasks = TASKS
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default();
            serde_json::to_value(tasks).map_err(|e| (500u16, e.to_string()))
        }

        _ => Err((
            if is_get || is_post { 404u16 } else { 405u16 },
            "接口不存在，GET / 查看接口列表".to_string(),
        )),
    }
}

/// 未登录 / 授权类错误返回 401，其余 500
fn auth_err(e: String) -> (u16, String) {
    if e.contains("登录") || e.contains("授权") {
        (401, e)
    } else {
        (500, e)
    }
}

// ---------- 分享接口辅助 ----------

/// 解析 POST 请求体 JSON
fn parse_json_body(body: &str) -> Result<Value, (u16, String)> {
    serde_json::from_str(body).map_err(|e| (400u16, format!("请求体不是合法 JSON: {e}")))
}

/// 取必填的非空字符串字段
fn req_str(v: &Value, key: &str, err: &str) -> Result<String, (u16, String)> {
    v[key]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or((400u16, err.to_string()))
}

/// 把网盘目录递归打包下载为 `<save_dir>/<name>.zip`（复用 download_zip）
async fn download_dir_zip(
    app: &AppHandle,
    netdisk_dir: &str,
    save_dir: &str,
    name: &str,
    id: String,
) -> Result<(), String> {
    tokio::fs::create_dir_all(save_dir)
        .await
        .map_err(|e| format!("创建目录失败: {e}"))?;
    let files = pan_api::list_files_recursive(&[netdisk_dir.to_string()]).await?;
    if files.is_empty() {
        return Err("目录中没有文件".into());
    }
    let prefix = netdisk_dir.trim_end_matches('/');
    let items: Vec<download::ZipItem> = files
        .iter()
        .map(|f| download::ZipItem {
            fs_id: f.fs_id,
            rel_path: f
                .path
                .strip_prefix(prefix)
                .unwrap_or(&f.path)
                .trim_start_matches('/')
                .to_string(),
            size: f.size,
        })
        .collect();
    let zip_path = format!("{save_dir}\\{name}.zip");
    let task_name = format!("目录打包：{name}");
    // download_zip 失败时会自行推送 error 事件并更新任务表
    download::download_zip(app.clone(), items, zip_path, task_name, id).await
}

// ---------- 查询串解析 ----------

fn parse_query(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                    out.push((h << 4) | l);
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
