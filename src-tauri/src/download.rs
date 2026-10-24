//! 流式下载：filemetas 获取 dlink -> 带 netdisk UA 跟随 302 下载。
//! 进度通过 Tauri 事件 `transfer://progress` 推送。
//! 支持单文件下载（自动创建父目录）与批量打包下载（先落临时目录再合成 zip）。

use crate::oauth;
use crate::pan_api::{self, PAN_UA};
use crate::upload::{emit, Progress};
use futures_util::future::join_all;
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// 进度事件节流间隔 / 下载自动重试次数
const EMIT_INTERVAL: Duration = Duration::from_millis(200);
const MAX_RETRIES: u32 = 3;
/// 暂停时的状态轮询间隔
const PAUSE_POLL: Duration = Duration::from_millis(150);
/// 分片并发参数：8 路连接、8MB 分片（低于该大小的文件走单连接）
const MAX_CONNECTIONS: usize = 8;
const PIECE_SIZE: u64 = 8 * 1024 * 1024;
const PARALLEL_MIN: u64 = PIECE_SIZE;
/// 分片进度落盘间隔（应用异常退出后仍可续传）
const STATE_SAVE_INTERVAL: Duration = Duration::from_secs(2);

/// 单个下载任务的控制状态（暂停标记），按任务 id 在进程内共享
#[derive(Default)]
struct DownloadCtrl {
    paused: AtomicBool,
}

impl DownloadCtrl {
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 暂停期间原地等待，恢复后返回
    async fn wait_resumed(&self) {
        while self.is_paused() {
            tokio::time::sleep(PAUSE_POLL).await;
        }
    }
}

fn controls() -> &'static Mutex<HashMap<String, Arc<DownloadCtrl>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<DownloadCtrl>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 注册下载任务并取得控制句柄（同 id 复用）
fn ctrl_for(id: &str) -> Arc<DownloadCtrl> {
    let mut map = controls().lock().unwrap();
    map.entry(id.to_string()).or_default().clone()
}

/// 任务结束后移除控制状态
fn release_ctrl(id: &str) {
    if let Ok(mut map) = controls().lock() {
        map.remove(id);
    }
}

/// 暂停 / 继续下载任务；返回任务是否存在（已结束的任务返回 false）
pub fn set_paused(id: &str, paused: bool) -> bool {
    let map = controls().lock().unwrap();
    match map.get(id) {
        Some(c) => {
            c.paused.store(paused, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// 下载过程中的错误：暂停不是失败，需要保留 `.part` 以便续传
enum DlErr {
    /// 用户暂停，附带已完成的字节数
    Paused(u64),
    Fatal(String),
}

impl From<String> for DlErr {
    fn from(s: String) -> Self {
        DlErr::Fatal(s)
    }
}

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

/// 并行分片进度文件：`xxx.part.state`（文本，进程异常退出后仍可续传）
fn state_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_os_string();
    s.push(".state");
    PathBuf::from(s)
}

/// 分片下载进度：done[i] 为第 i 片已写入的字节数
struct ParallelState {
    total: u64,
    piece_size: u64,
    done: Vec<u64>,
}

impl ParallelState {
    fn new(total: u64) -> Self {
        let pieces = total.div_ceil(PIECE_SIZE) as usize;
        Self {
            total,
            piece_size: PIECE_SIZE,
            done: vec![0; pieces],
        }
    }

    fn piece_len(&self, i: usize) -> u64 {
        let start = i as u64 * self.piece_size;
        (self.total - start).min(self.piece_size)
    }

    /// 状态是否自洽（分片数匹配、每片进度不越界）
    fn valid(&self) -> bool {
        self.piece_size > 0
            && self.done.len() as u64 == self.total.div_ceil(self.piece_size)
            && self
                .done
                .iter()
                .enumerate()
                .all(|(i, &d)| d <= self.piece_len(i))
    }

    fn complete(&self) -> bool {
        self.done
            .iter()
            .enumerate()
            .all(|(i, &d)| d >= self.piece_len(i))
    }

    fn total_done(&self) -> u64 {
        self.done.iter().sum()
    }
}

/// 读取状态文件；大小或分片参数不匹配时视为无效
fn load_state(part: &Path, total: u64) -> Option<ParallelState> {
    let text = std::fs::read_to_string(state_path(part)).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "v1" {
        return None;
    }
    let (t, p) = lines.next()?.trim().split_once(' ')?;
    let (t, p): (u64, u64) = (t.parse().ok()?, p.parse().ok()?);
    if t != total || p != PIECE_SIZE {
        return None;
    }
    let done: Vec<u64> = lines
        .next()?
        .split_whitespace()
        .map(|s| s.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    let st = ParallelState {
        total,
        piece_size: p,
        done,
    };
    st.valid().then_some(st)
}

fn save_state(part: &Path, st: &ParallelState) {
    let done: Vec<String> = st.done.iter().map(|d| d.to_string()).collect();
    let body = format!("v1\n{} {}\n{}\n", st.total, st.piece_size, done.join(" "));
    let _ = std::fs::write(state_path(part), body);
}

/// 本地已有进度：并行状态文件优先，否则按 `.part` 长度
fn local_done(dest: &Path) -> u64 {
    let part = part_path(dest);
    let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    match load_state(&part, len) {
        Some(st) if len > 0 => st.total_done(),
        _ => len,
    }
}

/// 准备分片状态：`xxx.part` 预分配为 total，各分片进度从状态文件恢复。
/// 先写状态再预分配，避免预分配后崩溃被误判为已完成。
async fn prepare_state(part: &Path, total: u64) -> Result<ParallelState, String> {
    let preallocated = tokio::fs::metadata(part)
        .await
        .map(|m| m.len() == total)
        .unwrap_or(false);
    let state = if preallocated {
        load_state(part, total).unwrap_or_else(|| ParallelState::new(total))
    } else {
        ParallelState::new(total)
    };
    save_state(part, &state);
    let f = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(part)
        .await
        .map_err(|e| format!("创建本地文件失败: {e}"))?;
    f.set_len(total)
        .await
        .map_err(|e| format!("预分配文件失败: {e}"))?;
    Ok(state)
}

/// 探测服务端是否支持 Range：206 返回远端总大小，200 表示不支持
async fn probe_range(url: &str) -> Result<Option<u64>, String> {
    let resp = http_client()
        .get(url)
        .header("User-Agent", PAN_UA)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 206 {
        return Ok(resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .map(|(_, total)| total));
    }
    if status.as_u16() == 200 {
        return Ok(None);
    }
    Err(format!("下载失败: HTTP {status}"))
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
/// 用户暂停时立即断开连接并保留 `.part`，继续时重新带 Range 请求续传。
async fn stream_download(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    fs_id: u64,
    dest: &Path,
    ctrl: &DownloadCtrl,
) -> Result<u64, String> {
    let mut attempt = 0;
    loop {
        // 暂停状态下不占用网络连接，等待用户点击继续
        if ctrl.is_paused() {
            let have = local_done(dest);
            emit(
                app,
                Progress {
                    kind: "download",
                    id: id.to_string(),
                    name: display_name.to_string(),
                    total: overall_total,
                    done: base_done + have,
                    phase: "paused",
                    message: "可点击继续断点续传".into(),
                    path: Some(dest.to_string_lossy().to_string()),
                },
            );
            ctrl.wait_resumed().await;
            emit(
                app,
                Progress {
                    kind: "download",
                    id: id.to_string(),
                    name: display_name.to_string(),
                    total: overall_total,
                    done: base_done + have,
                    phase: "downloading",
                    message: "继续下载…".into(),
                    path: Some(dest.to_string_lossy().to_string()),
                },
            );
        }
        attempt += 1;
        match download_attempt(app, id, display_name, base_done, overall_total, fs_id, dest, ctrl)
            .await
        {
            Ok(n) => return Ok(n),
            Err(DlErr::Paused(done)) => {
                // 暂停不算失败，不消耗重试次数
                attempt -= 1;
                emit(
                    app,
                    Progress {
                        kind: "download",
                        id: id.to_string(),
                        name: display_name.to_string(),
                        total: overall_total,
                        done: base_done + done,
                        phase: "paused",
                        message: "可点击继续断点续传".into(),
                        path: Some(dest.to_string_lossy().to_string()),
                    },
                );
            }
            Err(DlErr::Fatal(e)) if attempt <= MAX_RETRIES => {
                emit(
                    app,
                    Progress {
                        kind: "download",
                        id: id.to_string(),
                        name: display_name.to_string(),
                        total: overall_total,
                        done: base_done + local_done(dest),
                        phase: "downloading",
                        message: format!("{e}，{attempt} 秒后自动重试（断点续传）"),
                        path: Some(dest.to_string_lossy().to_string()),
                    },
                );
                tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
            }
            Err(DlErr::Fatal(e)) => return Err(e),
        }
    }
}

/// 单次下载尝试：大文件走多连接分片并发；小文件或服务端不支持 Range 时走单连接续传。
async fn download_attempt(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    fs_id: u64,
    dest: &Path,
    ctrl: &DownloadCtrl,
) -> Result<u64, DlErr> {
    let token = oauth::ensure_token().await?;
    let (dlink, size) = pan_api::file_dlink(fs_id).await?;
    let url = format!("{dlink}&access_token={token}");

    if size >= PARALLEL_MIN && !ctrl.is_paused() {
        match probe_range(&url).await? {
            Some(total) if total >= PARALLEL_MIN => {
                return download_parallel(
                    app,
                    id,
                    display_name,
                    base_done,
                    overall_total,
                    &url,
                    total,
                    dest,
                    ctrl,
                )
                .await;
            }
            // 服务端不支持 Range 或远端大小异常：清理分片残留，回退单连接
            _ => {
                let part = part_path(dest);
                let _ = tokio::fs::remove_file(&part).await;
                let _ = tokio::fs::remove_file(state_path(&part)).await;
            }
        }
    }
    download_sequential(
        app,
        id,
        display_name,
        base_done,
        overall_total,
        &url,
        dest,
        ctrl,
    )
    .await
}

/// 单连接下载：若本地已有 `.part` 文件则带 Range 请求续传。
async fn download_sequential(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    url: &str,
    dest: &Path,
    ctrl: &DownloadCtrl,
) -> Result<u64, DlErr> {
    let part = part_path(dest);
    let have: u64 = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);

    if ctrl.is_paused() {
        return Err(DlErr::Paused(have));
    }

    let mut req = http_client().get(url).header("User-Agent", PAN_UA);
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
        return Err(DlErr::Fatal("本地缓存异常，已重置".into()));
    }
    if !status.is_success() {
        return Err(DlErr::Fatal(format!("下载失败: HTTP {status}")));
    }

    // 206 = 续传生效（Content-Range 带起点与总长）；200 = 服务端不支持 Range，整段重下
    let (start, remote_len) = if status.as_u16() == 206 {
        resp.headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .ok_or_else(|| DlErr::Fatal("解析 Content-Range 失败".into()))?
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
        // 暂停：落盘已收到的数据后断开连接，继续时从 .part 续传
        if ctrl.is_paused() {
            file.flush().await.ok();
            file.sync_all().await.ok();
            return Err(DlErr::Paused(done));
        }
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
                    path: Some(dest.to_string_lossy().to_string()),
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

/// 分片并发下载的共享上下文
struct ParCtx<'a> {
    app: &'a AppHandle,
    id: &'a str,
    display_name: &'a str,
    base_done: u64,
    overall_total: u64,
    url: &'a str,
    part: &'a Path,
    dest: &'a Path,
    ctrl: &'a DownloadCtrl,
    total: u64,
    pieces: usize,
    state: Mutex<ParallelState>,
    /// 下一个待领取的分片序号
    next: AtomicUsize,
    /// 已写入的总字节数（用于进度与暂停位置）
    got: AtomicU64,
    /// 某个 worker 遇到致命错误时通知其他 worker 尽快退出
    stop: AtomicBool,
}

impl ParCtx<'_> {
    fn piece_len(&self, i: usize) -> u64 {
        let start = i as u64 * PIECE_SIZE;
        (self.total - start).min(PIECE_SIZE)
    }

    fn progress_total(&self) -> u64 {
        if self.overall_total > 0 {
            self.overall_total
        } else {
            self.base_done + self.total
        }
    }

    fn fatal(&self, msg: String) -> DlErr {
        self.stop.store(true, Ordering::SeqCst);
        DlErr::Fatal(msg)
    }
}

/// 单个分片 worker：从原子队列领取分片，按偏移写入 `.part`
async fn parallel_worker(ctx: &ParCtx<'_>, w: usize) -> Result<(), DlErr> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(ctx.part)
        .await
        .map_err(|e| format!("打开续传文件失败: {e}"))?;
    // 仅 0 号 worker 推送进度与落盘状态，避免事件风暴
    let mut last_emit = tokio::time::Instant::now();
    let mut last_got = ctx.got.load(Ordering::SeqCst);
    let mut last_save = tokio::time::Instant::now();

    loop {
        if ctx.stop.load(Ordering::SeqCst) {
            file.flush().await.ok();
            return Ok(());
        }
        if ctx.ctrl.is_paused() {
            file.flush().await.ok();
            return Err(DlErr::Paused(ctx.got.load(Ordering::SeqCst)));
        }
        let i = ctx.next.fetch_add(1, Ordering::SeqCst);
        if i >= ctx.pieces {
            file.flush().await.ok();
            return Ok(());
        }
        let start = i as u64 * PIECE_SIZE;
        let len = ctx.piece_len(i);
        loop {
            let have = match ctx.state.lock() {
                Ok(st) => st.done[i],
                Err(_) => return Err(ctx.fatal("分片状态异常".into())),
            };
            if have >= len {
                break;
            }
            let from = start + have;
            let to = start + len - 1;
            let resp = http_client()
                .get(ctx.url)
                .header("User-Agent", PAN_UA)
                .header(reqwest::header::RANGE, format!("bytes={from}-{to}"))
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => return Err(ctx.fatal(format!("下载请求失败: {e}"))),
            };
            if resp.status().as_u16() != 206 {
                return Err(ctx.fatal(format!("分片下载失败: HTTP {}", resp.status())));
            }

            let mut pos = from;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if ctx.stop.load(Ordering::SeqCst) {
                    file.flush().await.ok();
                    return Ok(());
                }
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => return Err(ctx.fatal(format!("下载中断: {e}"))),
                };
                // 暂停：保留已写进度，立即断开连接
                if ctx.ctrl.is_paused() {
                    file.flush().await.ok();
                    file.sync_all().await.ok();
                    if let Ok(st) = ctx.state.lock() {
                        save_state(ctx.part, &st);
                    }
                    return Err(DlErr::Paused(ctx.got.load(Ordering::SeqCst)));
                }
                if let Err(e) = file.seek(SeekFrom::Start(pos)).await {
                    return Err(ctx.fatal(format!("写入文件失败: {e}")));
                }
                if let Err(e) = file.write_all(&chunk).await {
                    return Err(ctx.fatal(format!("写入文件失败: {e}")));
                }
                pos += chunk.len() as u64;
                ctx.got.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                if let Ok(mut st) = ctx.state.lock() {
                    st.done[i] = pos - start;
                }
                if w != 0 {
                    continue;
                }
                let now = tokio::time::Instant::now();
                if now.duration_since(last_emit) >= EMIT_INTERVAL {
                    let got = ctx.got.load(Ordering::SeqCst);
                    let el = now.duration_since(last_emit).as_secs_f64();
                    let message = if el > 0.0 {
                        format!("{}/s", fmt_size(got.saturating_sub(last_got) as f64 / el))
                    } else {
                        String::new()
                    };
                    emit(
                        ctx.app,
                        Progress {
                            kind: "download",
                            id: ctx.id.to_string(),
                            name: ctx.display_name.to_string(),
                            total: ctx.progress_total(),
                            done: ctx.base_done + got,
                            phase: "downloading",
                            message,
                            path: Some(ctx.dest.to_string_lossy().to_string()),
                        },
                    );
                    last_emit = now;
                    last_got = got;
                }
                if now.duration_since(last_save) >= STATE_SAVE_INTERVAL {
                    if let Ok(st) = ctx.state.lock() {
                        save_state(ctx.part, &st);
                    }
                    last_save = now;
                }
            }
            // 服务端提前结束响应：同一分片继续请求剩余部分
            if pos == from {
                return Err(ctx.fatal("分片下载无数据".into()));
            }
        }
    }
}

/// 多连接分片并发下载：`Range` 请求 8MB 分片、最多 8 路并发，预分配后按偏移写入。
async fn download_parallel(
    app: &AppHandle,
    id: &str,
    display_name: &str,
    base_done: u64,
    overall_total: u64,
    url: &str,
    total: u64,
    dest: &Path,
    ctrl: &DownloadCtrl,
) -> Result<u64, DlErr> {
    let part = part_path(dest);
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    let state = prepare_state(&part, total).await?;
    let pieces = state.done.len();
    let ctx = ParCtx {
        app,
        id,
        display_name,
        base_done,
        overall_total,
        url,
        part: &part,
        dest,
        ctrl,
        total,
        pieces,
        state: Mutex::new(state),
        next: AtomicUsize::new(0),
        got: AtomicU64::new(0),
        stop: AtomicBool::new(false),
    };
    if let Ok(st) = ctx.state.lock() {
        ctx.got.store(st.total_done(), Ordering::SeqCst);
    }

    let workers = pieces.min(MAX_CONNECTIONS);
    let results = join_all((0..workers).map(|w| parallel_worker(&ctx, w))).await;

    let mut paused = false;
    let mut fatal: Option<String> = None;
    for r in results {
        match r {
            Ok(()) => {}
            Err(DlErr::Paused(_)) => paused = true,
            Err(DlErr::Fatal(e)) => {
                if fatal.is_none() {
                    fatal = Some(e);
                }
            }
        }
    }

    // 暂停 / 失败 / 完成都落盘一次，确保下次可续传
    let complete = match ctx.state.lock() {
        Ok(st) => {
            save_state(&part, &st);
            st.complete()
        }
        Err(_) => false,
    };
    if paused {
        return Err(DlErr::Paused(ctx.got.load(Ordering::SeqCst)));
    }
    if let Some(e) = fatal {
        return Err(DlErr::Fatal(e));
    }
    if !complete {
        return Err(DlErr::Fatal("分片下载不完整".into()));
    }

    if let Ok(f) = tokio::fs::OpenOptions::new().read(true).open(&part).await {
        f.sync_all().await.ok();
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("保存文件失败: {e}"))?;
    let _ = tokio::fs::remove_file(state_path(&part)).await;
    Ok(total)
}

pub async fn download_file(
    app: AppHandle,
    fs_id: u64,
    save_path: String,
    name: String,
) -> Result<(), String> {
    let id = format!("d:{fs_id}");
    let ctrl = ctrl_for(&id);
    let result = stream_download(
        &app,
        &id,
        &name,
        0,
        0,
        fs_id,
        Path::new(&save_path),
        ctrl.as_ref(),
    )
    .await;
    release_ctrl(&id);
    let done = result?;
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
            path: Some(save_path),
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
    let ctrl = ctrl_for(&id);
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
                    path: Some(save_path.clone()),
                },
            );
            let dest = temp_root.join(&it.rel_path);
            base += stream_download(&app, &id, &task_name, base, total, it.fs_id, &dest, ctrl.as_ref())
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
                path: Some(save_path.clone()),
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

    release_ctrl(&id);
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
            path: Some(save_path),
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
