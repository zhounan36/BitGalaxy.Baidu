//! 流式下载：filemetas 获取 dlink -> 带 netdisk UA 跟随 302 下载。
//! 进度通过 Tauri 事件 `transfer://progress` 推送。
//! 支持单文件下载（自动创建父目录）与批量打包下载（先落临时目录再合成 zip）。

use crate::oauth;
use crate::pan_api::{self, PAN_UA};
use crate::upload::{emit, Progress};
use futures_util::StreamExt;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;

/// 进度事件节流间隔 / 下载自动重试次数
const EMIT_INTERVAL: Duration = Duration::from_millis(200);
const MAX_RETRIES: u32 = 3;

pub fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 复用全局 HTTP 客户端，批量下载时避免重复 TLS 握手
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client 构建失败")
    })
}

/// 续传临时文件路径：`xxx.part`
fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// 解析 `Content-Range: bytes start-end/total`
fn parse_content_range(v: &str) -> Option<(u64, u64)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.parse().ok()?;
    let total = total.parse().ok()?;
    Some((start, total))
}

/// 字节数人性化显示（实时速度用）
fn fmt_size(n: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{:.0} B", n)
    }
}

/// 流式下载单个网盘文件到本地路径（自动创建父目录），返回实际下载大小。
/// 进度事件按整体任务汇总：id / display_name / base_done（此文件前的累计）/ overall_total。
/// 数据先落 `<dest>.part`，完成后再改名为正式文件；中断后自动重试并从断点续传。
async fn stream_download(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    fs_id: u64,
    dest: &Path,
) -> Result<u64, String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match download_attempt(app, id, display_name, base_done, overall_total, fs_id, dest).await {
            Ok(n) => return Ok(n),
            Err(e) if attempt <= MAX_RETRIES => {
                emit(
                    app,
                    Progress {
                        kind: "download",
                        id: id.to_string(),
                        name: display_name.to_string(),
                        total: overall_total,
                        done: base_done,
                        phase: "downloading",
                        message: format!("{e}，{attempt} 秒后自动重试（断点续传）"),
                    },
                );
                tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 单次下载尝试：若本地已有 `.part` 文件则带 Range 请求续传。
async fn download_attempt(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    fs_id: u64,
    dest: &Path,
) -> Result<u64, String> {
    let token = oauth::ensure_token().await?;
    let dlink = pan_api::file_dlink(fs_id).await?;
    let url = format!("{dlink}&access_token={token}");

    let part = part_path(dest);
    let have: u64 = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);

    let mut req = http_client().get(&url).header("User-Agent", PAN_UA);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| format!("下载请求失败: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 416 {
        // 本地 .part 长度已达远端大小：恰好完整则直接收尾，否则重置重下
        let remote_total = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.trim().parse::<u64>().ok());
        if have > 0 && remote_total == Some(have) {
            tokio::fs::rename(&part, dest)
                .await
                .map_err(|e| format!("保存文件失败: {e}"))?;
            return Ok(have);
        }
        let _ = tokio::fs::remove_file(&part).await;
        return Err("本地缓存异常，已重置".into());
    }
    if !status.is_success() {
        return Err(format!("下载失败: HTTP {status}"));
    }

    // 206 = 续传生效（Content-Range 带起点与总长）；200 = 服务端不支持 Range，整段重下
    let (start, remote_len) = if status.as_u16() == 206 {
        resp.headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .ok_or("解析 Content-Range 失败")?
    } else {
        let len = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        (0u64, len)
    };
    let total = if overall_total > 0 {
        overall_total
    } else {
        remote_len
    };

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    let mut file = if start > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await
            .map_err(|e| format!("打开续传文件失败: {e}"))?
    } else {
        tokio::fs::File::create(&part)
            .await
            .map_err(|e| format!("创建本地文件失败: {e}"))?
    };

    let mut done: u64 = start;
    let mut speed = String::new();
    let mut last_at = tokio::time::Instant::now();
    let mut last_bytes = start;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入文件失败: {e}"))?;
        done += chunk.len() as u64;
        // 进度事件节流推送，同时附带实时速度
        let now = tokio::time::Instant::now();
        if now.duration_since(last_at) >= EMIT_INTERVAL {
            let el = now.duration_since(last_at).as_secs_f64();
            if el > 0.0 {
                speed = format!("{}/s", fmt_size((done - last_bytes) as f64 / el));
            }
            emit(
                app,
                Progress {
                    kind: "download",
                    id: id.to_string(),
                    name: display_name.to_string(),
                    total: if total > 0 { total } else { base_done + done },
                    done: base_done + done,
                    phase: "downloading",
                    message: speed.clone(),
                },
            );
            last_at = now;
            last_bytes = done;
        }
    }
    file.flush().await.map_err(|e| format!("写入文件失败: {e}"))?;
    file.sync_all().await.ok();
    drop(file); // Windows 下 rename 前必须先关闭句柄
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("保存文件失败: {e}"))?;
    Ok(done)
}

pub async fn download_file(
    app: AppHandle,
    fs_id: u64,
    save_path: String,
    name: String,
) -> Result<(), String> {
    let id = format!("d:{fs_id}");
    let done = stream_download(&app, &id, &name, 0, 0, fs_id, Path::new(&save_path)).await?;
    emit(
        &app,
        Progress {
            kind: "download",
            id,
            name,
            total: done,
            done,
            phase: "done",
            message: String::new(),
        },
    );
    Ok(())
}

/// 打包下载项：rel_path 为压缩包内的相对路径（含文件名）
#[derive(Deserialize)]
pub struct ZipItem {
    pub fs_id: u64,
    pub rel_path: String,
    pub size: u64,
}

/// 批量打包下载：串行下载到临时目录（保持相对路径）-> 合成 zip -> 保存并清理。
/// 以单任务形式推送整体进度（phase: downloading / packing / done / error）。
/// `id` 由调用方生成（保证 API 与前端可用同一任务号查询进度）。
pub async fn download_zip(
    app: AppHandle,
    items: Vec<ZipItem>,
    save_path: String,
    task_name: String,
    id: String,
) -> Result<(), String> {
    if items.is_empty() {
        return Err("没有可打包的文件".into());
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    let bar_total = if total > 0 { total } else { 1 };
    let temp_root = std::env::temp_dir().join(format!("baidupan-zip-{}", now_ms()));
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|e| format!("创建临时目录失败: {e}"))?;

    let n = items.len();
    let mut base: u64 = 0;
    let result: Result<(), String> = async {
        for (i, it) in items.iter().enumerate() {
            emit(
                &app,
                Progress {
                    kind: "download",
                    id: id.clone(),
                    name: format!("{task_name}（{}/{n}）", i + 1),
                    total: bar_total,
                    done: base,
                    phase: "downloading",
                    message: format!("正在下载 {}/{}：{}", i + 1, n, it.rel_path),
                },
            );
            let dest = temp_root.join(&it.rel_path);
            base += stream_download(&app, &id, &task_name, base, total, it.fs_id, &dest)
                .await
                .map_err(|e| format!("{}：{e}", it.rel_path))?;
        }
        emit(
            &app,
            Progress {
                kind: "download",
                id: id.clone(),
                name: task_name.clone(),
                total: bar_total,
                done: bar_total,
                phase: "packing",
                message: "正在打包…".into(),
            },
        );
        let src = temp_root.clone();
        let dst = save_path.clone();
        tokio::task::spawn_blocking(move || build_zip(&src, &dst))
            .await
            .map_err(|e| format!("打包任务异常: {e}"))?
            .map_err(|e| format!("打包失败: {e}"))?;
        Ok(())
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&temp_root).await;
    let (phase, message) = match &result {
        Ok(()) => ("done", "打包完成".to_string()),
        Err(e) => ("error", e.clone()),
    };
    emit(
        &app,
        Progress {
            kind: "download",
            id,
            name: task_name,
            total: bar_total,
            done: if phase == "done" { bar_total } else { base },
            phase,
            message,
        },
    );
    result
}

/// 递归把 src 目录打成 Stored zip（仅打包不压缩，速度快）
fn build_zip(src: &Path, dest: &str) -> Result<(), String> {
    let file = std::fs::File::create(dest).map_err(|e| format!("创建压缩包失败: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    let mut stack: Vec<PathBuf> = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取临时目录失败: {e}"))?;
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p
                .strip_prefix(src)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                zip.add_directory(name, opts)
                    .map_err(|e| format!("写入目录条目失败: {e}"))?;
                stack.push(p);
            } else {
                zip.start_file(name, opts)
                    .map_err(|e| format!("写入压缩包失败: {e}"))?;
                let mut f = std::fs::File::open(&p).map_err(|e| format!("读取文件失败: {e}"))?;
                std::io::copy(&mut f, &mut zip)
                    .map_err(|e| format!("写入压缩包失败: {e}"))?;
            }
        }
    }
    zip.finish().map_err(|e| format!("完成压缩包失败: {e}"))?;
    Ok(())
}
