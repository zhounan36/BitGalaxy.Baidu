//! 百度网盘开放平台 xpan 文件管理接口封装。
//! 文档：https://pan.baidu.com/union/doc/

use crate::oauth;
use serde::Serialize;
use serde_json::{json, Value};

const HOST: &str = "https://pan.baidu.com";
/// xpan 文件接口前缀（容量等接口在 /api 下，不在 xpan 下）
const XPAN: &str = "/rest/2.0/xpan";
/// 百度 PCS 要求的 UA，否则下载等接口返回 403
pub const PAN_UA: &str = "netdisk";

#[derive(Serialize, Clone, Default)]
pub struct FileItem {
    pub fs_id: u64,
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
    pub category: i64,
}

pub fn parse_item(v: &Value) -> FileItem {
    let path = v["path"].as_str().unwrap_or_default().to_string();
    let name = v["server_filename"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            path.rsplit('/').next().unwrap_or_default().to_string()
        });
    FileItem {
        fs_id: v["fs_id"].as_u64().unwrap_or(0),
        name,
        is_dir: v["isdir"].as_i64().unwrap_or(0) == 1,
        size: v["size"].as_u64().unwrap_or(0),
        mtime: v["server_mtime"].as_u64().unwrap_or(0),
        category: v["category"].as_i64().unwrap_or(0),
        path,
    }
}

/// 统一的百度 API 错误检查
fn check(v: &Value) -> Result<(), String> {
    if let Some(code) = v.get("error_code").and_then(|c| c.as_i64()) {
        if code != 0 {
            let msg = v["error_msg"].as_str().unwrap_or("未知错误");
            let hint = match code {
                -6 => "身份过期，请重新授权",
                31034 => "接口命中黑名单/配额不足，请更换自己的 AppKey",
                111 => "access_token 过期，请重新授权",
                _ => msg,
            };
            return Err(format!("[{code}] {hint}"));
        }
    }
    if let Some(errno) = v.get("errno").and_then(|c| c.as_i64()) {
        if errno != 0 {
            return Err(format!("[errno {errno}] 请求失败"));
        }
    }
    Ok(())
}

async fn api_get(path_and_query: &str) -> Result<Value, String> {
    let token = oauth::ensure_token().await?;
    let url = format!("{HOST}{path_and_query}&access_token={token}");
    let v: Value = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", PAN_UA)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    check(&v)?;
    Ok(v)
}

async fn api_post(path_and_query: &str, form: &[(&str, String)]) -> Result<Value, String> {
    let token = oauth::ensure_token().await?;
    let url = format!("{HOST}{XPAN}{path_and_query}&access_token={token}");
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let v: Value = reqwest::Client::new()
        .post(&url)
        .header("User-Agent", PAN_UA)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;
    check(&v)?;
    Ok(v)
}

/// 用户信息
pub async fn uinfo() -> Result<Value, String> {
    api_get("/rest/2.0/xpan/nas?method=uinfo").await
}

/// 网盘容量（注意：此接口在 /api/quota，不在 xpan 路径下）
pub async fn quota() -> Result<Value, String> {
    api_get("/api/quota?checkfree=1&checkexpire=1").await
}

/// 列出目录内容（分页）
pub async fn list_dir(dir: &str, order: &str, desc: bool, start: u32, limit: u32) -> Result<Vec<FileItem>, String> {
    let q = format!(
        "{XPAN}/file?method=list&web=1&dir={}&order={}&desc={}&start={}&limit={}",
        urlencode(dir),
        order,
        if desc { 1 } else { 0 },
        start,
        limit
    );
    let v = api_get(&q).await?;
    let list = v["list"].as_array().cloned().unwrap_or_default();
    Ok(list.iter().map(parse_item).collect())
}

/// 递归搜索
pub async fn search(key: &str, dir: &str) -> Result<Vec<FileItem>, String> {
    let q = format!(
        "{XPAN}/file?method=search&web=1&recursion=1&key={}&dir={}",
        urlencode(key),
        urlencode(dir)
    );
    let v = api_get(&q).await?;
    let list = v["list"].as_array().cloned().unwrap_or_default();
    Ok(list.iter().map(parse_item).collect())
}

/// 删除文件（进回收站）
pub async fn delete(paths: &[String]) -> Result<(), String> {
    let filelist = serde_json::to_string(paths).unwrap();
    api_post("/file?method=delete", &[("filelist", filelist)]).await?;
    Ok(())
}

/// 重命名
pub async fn rename(path: &str, new_name: &str) -> Result<(), String> {
    let filelist = json!([{ "path": path, "newname": new_name }]).to_string();
    let v = api_post(
        "/file?method=filemanager&async=2&opera=rename",
        &[("filelist", filelist)],
    )
    .await?;
    // async=2 时检查 info 数组内的 errno
    if let Some(info) = v["info"].as_array() {
        for it in info {
            if it["errno"].as_i64().unwrap_or(0) != 0 {
                return Err(format!("重命名失败 [errno {}]", it["errno"].as_i64().unwrap_or(-1)));
            }
        }
    }
    Ok(())
}

/// 新建目录
pub async fn mkdir(path: &str) -> Result<(), String> {
    let filelist = json!([{ "path": path, "isdir": 1, "size": 0 }]).to_string();
    api_post(
        "/file?method=filemanager&async=2&opera=create",
        &[("filelist", filelist)],
    )
    .await?;
    Ok(())
}

/// 递归展开目录：返回其中所有文件（目录不入列），按路径排序。限制单次最多 3000 个文件。
pub async fn list_files_recursive(dirs: &[String]) -> Result<Vec<FileItem>, String> {
    const PAGE: u32 = 100;
    const MAX_FILES: usize = 3000;
    let mut out = Vec::new();
    let mut queue: std::collections::VecDeque<String> = dirs.iter().cloned().collect();
    while let Some(dir) = queue.pop_front() {
        let mut start: u32 = 0;
        loop {
            let list = list_dir(&dir, "name", false, start, PAGE).await?;
            let got = list.len() as u32;
            for it in list {
                if it.is_dir {
                    queue.push_back(it.path);
                } else {
                    out.push(it);
                    if out.len() > MAX_FILES {
                        return Err(format!("文件数超过 {MAX_FILES}，请分批下载"));
                    }
                }
            }
            if got < PAGE {
                break;
            }
            start += PAGE;
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// 获取文件下载直链 dlink 与文件大小（并行分片下载需要总长度）
pub async fn file_dlink(fs_id: u64) -> Result<(String, u64), String> {
    let q = format!("{XPAN}/multimedia?method=filemetas&dlink=1&fsids=[{fs_id}]");
    let v = api_get(&q).await?;
    let dlink = v["list"][0]["dlink"].as_str().unwrap_or_default().to_string();
    if dlink.is_empty() {
        return Err("获取下载链接失败（目录不支持下载）".into());
    }
    let size = v["list"][0]["size"].as_u64().unwrap_or(0);
    Ok((dlink, size))
}

pub fn urlencode(s: &str) -> String {
    // 只对路径类参数转义，避免手写完整 percent-encoding 依赖
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
