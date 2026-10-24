//! 分片上传：预上传(precreate) -> 分片上传(superfile2) -> 创建文件(create)。
//! 固定 4MB 分片，支持秒传；进度通过 Tauri 事件 `transfer://progress` 推送。
//! 内存策略：先流式计算分片 md5，再按需 seek 读取待传分片，峰值约 4MB。

use crate::oauth;
use crate::pan_api::{self, PAN_UA};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tauri::{AppHandle, Emitter};

const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const PCS_SUPERFILE: &str = "https://d.pcs.baidu.com/rest/2.0/pcs/superfile2";
const PRECREATE_URL: &str = "https://pan.baidu.com/rest/2.0/xpan/file?method=precreate";
const CREATE_URL: &str = "https://pan.baidu.com/rest/2.0/xpan/file?method=create";

#[derive(Serialize, Clone)]
pub struct Progress {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub total: u64,
    pub done: u64,
    /// hashing / uploading / merging / done / error
    pub phase: &'static str,
    pub message: String,
    /// 下载任务的目标文件路径（前端用于打开所在文件夹）；上传任务不携带
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub fn emit(app: &AppHandle, p: Progress) {
    let _ = app.emit("transfer://progress", &p);
}

/// 第一遍：流式计算全部分片 md5（内存只占一个分片）
fn calc_block_md5s(f: &mut std::fs::File, size: u64) -> Result<Vec<String>, String> {
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut blocks = Vec::new();
    let mut remain = size;
    while remain > 0 {
        let n = remain.min(CHUNK_SIZE) as usize;
        let mut buf = vec![0u8; n];
        f.read_exact(&mut buf).map_err(|e| format!("读取文件失败: {e}"))?;
        let mut h = Md5::new();
        h.update(&buf);
        let md5: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        blocks.push(md5);
        remain -= n as u64;
    }
    Ok(blocks)
}

/// 第二遍：按索引读取一个分片
fn read_chunk(f: &mut std::fs::File, size: u64, idx: u64) -> Result<Vec<u8>, String> {
    let offset = idx * CHUNK_SIZE;
    if offset >= size {
        return Err("分片索引越界".into());
    }
    let n = (size - offset).min(CHUNK_SIZE) as usize;
    let mut buf = vec![0u8; n];
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.read_exact(&mut buf).map_err(|e| format!("读取文件失败: {e}"))?;
    Ok(buf)
}

pub async fn upload_files(
    app: AppHandle,
    local_paths: Vec<String>,
    remote_dir: String,
) -> Result<(), String> {
    let token = oauth::ensure_token().await?;
    let dir = if remote_dir.ends_with('/') {
        remote_dir
    } else {
        format!("{remote_dir}/")
    };
    for local in &local_paths {
        let name = Path::new(local)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local.clone());
        if let Err(e) = upload_one(&app, &token, local, &name, &dir).await {
            emit(
                &app,
                Progress {
                    kind: "upload",
                    id: format!("u:{local}"),
                    name,
                    total: 0,
                    done: 0,
                    phase: "error",
                    message: e,
                    path: None,
                },
            );
        }
    }
    Ok(())
}

async fn upload_one(
    app: &AppHandle,
    token: &str,
    local: &str,
    name: &str,
    dir: &str,
) -> Result<(), String> {
    let path = Path::new(local);
    let id = format!("u:{local}");
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件信息失败: {e}"))?
        .len();
    let remote_path = format!("{dir}{name}");

    let mut file = std::fs::File::open(path).map_err(|e| format!("打开文件失败: {e}"))?;
    let block_md5s = calc_block_md5s(&mut file, size)?;
    let block_list_json = serde_json::to_string(&block_md5s).unwrap();

    emit(app, Progress { kind: "upload", id: id.clone(), name: name.to_string(), total: size, done: 0, phase: "hashing", message: String::new(), path: None });

    // 1. 预上传
    let pre = form_post(
        &format!("{PRECREATE_URL}&access_token={token}"),
        &[
            ("path", remote_path.clone()),
            ("size", size.to_string()),
            ("isdir", "0".into()),
            ("autoinit", "1".into()),
            ("rtype", "3".into()),
            ("block_list", block_list_json.clone()),
        ],
    )
    .await?;
    if pre.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0) != 0 {
        return Err(format!(
            "预上传失败: {} {}",
            pre["error_code"], pre["error_msg"]
        ));
    }
    if pre["return_type"].as_i64() == Some(2) {
        emit(app, Progress { kind: "upload", id, name: name.to_string(), total: size, done: size, phase: "done", message: "秒传成功".into(), path: None });
        return Ok(());
    }
    let uploadid = pre["uploadid"].as_str().unwrap_or_default().to_string();
    // 需要上传的分片索引（为空则全部上传）
    let need: Vec<u64> = {
        let listed = pre["block_list"].as_array();
        match listed {
            Some(a) if !a.is_empty() => a.iter().filter_map(|x| x.as_u64()).collect(),
            _ => (0..block_md5s.len() as u64).collect(),
        }
    };

    // 2. 分片上传
    let mut done: u64 = 0;
    for idx in need {
        let chunk = read_chunk(&mut file, size, idx)?;
        let url = format!(
            "{PCS_SUPERFILE}?access_token={token}&method=upload&type=tmpfile&path={}&uploadid={uploadid}&partseq={idx}",
            pan_api::urlencode(&remote_path)
        );
        let part = reqwest::multipart::Part::bytes(chunk.clone())
            .file_name(format!("chunk_{idx}"))
            .mime_str("application/octet-stream")
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let v: Value = reqwest::Client::new()
            .put(&url)
            .header("User-Agent", PAN_UA)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("分片上传失败: {e}"))?
            .json()
            .await
            .map_err(|e| format!("分片上传响应解析失败: {e}"))?;
        if v.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0) != 0 {
            return Err(format!("分片上传失败: {} {}", v["error_code"], v["error_msg"]));
        }
        done += chunk.len() as u64;
        emit(app, Progress { kind: "upload", id: id.clone(), name: name.to_string(), total: size, done, phase: "uploading", message: String::new(), path: None });
    }

    // 3. 创建文件（合并分片）
    emit(app, Progress { kind: "upload", id: id.clone(), name: name.to_string(), total: size, done, phase: "merging", message: String::new(), path: None });
    let created = form_post(
        &format!("{CREATE_URL}&access_token={token}"),
        &[
            ("path", remote_path),
            ("size", size.to_string()),
            ("isdir", "0".into()),
            ("uploadid", uploadid),
            ("rtype", "3".into()),
            ("block_list", block_list_json),
        ],
    )
    .await?;
    if created.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0) != 0 {
        return Err(format!(
            "创建文件失败: {} {}",
            created["error_code"], created["error_msg"]
        ));
    }
    emit(app, Progress { kind: "upload", id, name: name.to_string(), total: size, done: size, phase: "done", message: String::new(), path: None });
    Ok(())
}

async fn form_post(url: &str, form: &[(&str, String)]) -> Result<Value, String> {
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let v: Value = reqwest::Client::new()
        .post(url)
        .header("User-Agent", PAN_UA)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    Ok(v)
}
