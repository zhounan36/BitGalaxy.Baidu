//! 分享链接解析、转存与二维码截图读取。
//!
//! 开放平台未提供「解析任意分享链接」的接口，因此解析复用网页端匿名会话：
//!   1. GET  /s/{surl}            —— 公开分享页面内嵌 shareid / uk
//!   2. POST /share/verify        —— 私密分享先校验提取码，得到 randsk(sekey)，写入 BDCLND cookie
//!   3. GET  /share/list          —— 拉取分享根目录文件列表（失败时回退解析页面内嵌列表）
//!   4. POST xpan share/transfer  —— 官方接口转存到自己网盘，拿到转存后的 fs_id
//!   5. 前端复用 download_file 命令下载转存后的文件

use crate::oauth;
use crate::pan_api::{self, FileItem};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[derive(Serialize, Default)]
pub struct ShareInfo {
    /// true 表示该分享需要提取码，用户补填后重试
    pub need_pwd: bool,
    pub shareid: String,
    pub uk: String,
    /// 私密分享校验提取码后的 key（URL 编码态），转存时需要
    pub sekey: String,
    pub files: Vec<FileItem>,
}

#[derive(Serialize)]
pub struct TransferResult {
    /// 分享中原文件的 fs_id
    pub from_fs_id: u64,
    /// 转存到自己网盘后的新 fs_id（0 表示接口未返回）
    pub fs_id: u64,
    /// 转存后的网盘路径
    pub path: String,
}

/// 从任意粘贴文本中提取（完整 surl，链接内提取码）。
/// 支持 /s/1xxxx、share/init?surl=xxxx 与裸 surl 三种形式。
fn extract_surl(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    // ?pwd=xxxx
    let pwd = t
        .split("pwd=")
        .nth(1)
        .map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .take(4)
                .collect::<String>()
        })
        .filter(|s| s.len() == 4)
        .unwrap_or_default();

    let take_code = |rest: &str| -> String {
        rest.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    };
    // /s/1AbCdEf
    if let Some(i) = t.find("/s/") {
        let s = take_code(&t[i + 3..]);
        if !s.is_empty() {
            return Some((s, pwd));
        }
    }
    // share/init?surl=AbCdEf（init 链接的 surl 不含前导 1）
    if let Some(i) = t.find("surl=") {
        let s = take_code(&t[i + 5..]);
        if !s.is_empty() {
            return Some((format!("1{s}"), pwd));
        }
    }
    // 裸 surl
    if t.len() >= 8 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        let full = if t.starts_with('1') {
            t.to_string()
        } else {
            format!("1{t}")
        };
        return Some((full, pwd));
    }
    None
}

/// 从 HTML 中提取 `"key":"123"` / `"key":123` 形式的数字字段
fn extract_field(html: &str, key: &str) -> String {
    let pat = format!("\"{key}\":");
    let Some(i) = html.find(&pat) else {
        return String::new();
    };
    let rest = html[i + pat.len()..].trim_start();
    let mut chars = rest.chars();
    match chars.next() {
        Some('"') => chars.take_while(|c| *c != '"').collect(),
        Some(c) if c.is_ascii_digit() => {
            let mut s = c.to_string();
            s.extend(chars.take_while(|c| c.is_ascii_digit()));
            s
        }
        _ => String::new(),
    }
}

/// 从 start（指向 '[' 或 '{'）扫描到配对结束，返回该 JSON 片段
fn slice_json(s: &str, start: usize) -> Option<String> {
    let bytes = s.as_bytes();
    let open = *bytes.get(start)?;
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            _ if b == open => depth += 1,
            _ if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// 回退方案：直接解析分享页面内嵌的文件列表 JSON
fn extract_files_from_html(html: &str) -> Option<Vec<FileItem>> {
    for pat in ["\"file_list\"", "\"list\""] {
        let mut from = 0usize;
        while let Some(rel) = html[from..].find(pat) {
            let at = from + rel + pat.len();
            let rest = &html[at..];
            let trimmed = rest.trim_start();
            let off = at + (rest.len() - trimmed.len());
            match trimmed.as_bytes().first() {
                Some(b'{') => {
                    // file_list 是对象，其内再找 list 数组
                    if let Some(obj) = slice_json(html, off) {
                        if let Ok(v) = serde_json::from_str::<Value>(&obj) {
                            if let Some(list) = v["list"].as_array() {
                                let items: Vec<FileItem> =
                                    list.iter().map(pan_api::parse_item).collect();
                                if !items.is_empty() {
                                    return Some(items);
                                }
                            }
                        }
                    }
                }
                Some(b'[') => {
                    if let Some(arr) = slice_json(html, off) {
                        if let Ok(v) = serde_json::from_str::<Value>(&arr) {
                            if let Some(list) = v.as_array() {
                                if list.iter().any(|i| i.get("fs_id").is_some()) {
                                    return Some(list.iter().map(pan_api::parse_item).collect());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            from = at;
        }
    }
    None
}

async fn fetch_share_page(client: &reqwest::Client, surl: &str) -> Result<String, String> {
    let url = format!("https://pan.baidu.com/s/{surl}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("访问分享页面失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        // 404 等情况 reqwest 不会抛错，这里统一转成友好提示
        if status.as_u16() == 404 {
            return Err("分享链接不存在或已失效".into());
        }
        return Err(format!("访问分享页面失败: HTTP {status}"));
    }
    resp.text()
        .await
        .map_err(|e| format!("读取分享页面失败: {e}"))
}

/// 校验私密分享提取码，成功返回 randsk(sekey)
async fn verify_password(
    client: &reqwest::Client,
    surl_code: &str,
    pwd: &str,
) -> Result<String, String> {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let url = format!(
        "https://pan.baidu.com/share/verify?surl={surl_code}&t={t}&channel=chunlei&web=1&clienttype=0"
    );
    let v: Value = client
        .post(&url)
        .header("Referer", format!("https://pan.baidu.com/share/init?surl={surl_code}"))
        .form(&[("pwd", pwd), ("vcode", "")])
        .send()
        .await
        .map_err(|e| format!("提取码校验请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("提取码校验响应解析失败: {e}"))?;
    match v["errno"].as_i64() {
        Some(0) => Ok(v["randsk"].as_str().unwrap_or_default().to_string()),
        Some(-9) => Err("提取码错误".into()),
        Some(-62) | Some(-12) => {
            Err("触发人机验证，请在浏览器中打开一次该分享后重试".into())
        }
        Some(e) => Err(format!("提取码校验失败 [errno {e}]")),
        None => Err("提取码校验失败：响应异常".into()),
    }
}

/// 拉取分享根目录文件列表（share/list 优先，失败尝试 wxlist）
async fn share_list(
    client: &reqwest::Client,
    shareid: &str,
    uk: &str,
) -> Result<Vec<FileItem>, String> {
    let urls = [
        format!("https://pan.baidu.com/share/list?shareid={shareid}&uk={uk}&page=1&num=100&root=1&web=1"),
        format!("https://pan.baidu.com/share/wxlist?channel=weixin&version=2.9.6&web=1&shareid={shareid}&uk={uk}&root=1"),
    ];
    let mut last_err = String::new();
    for url in urls {
        let Ok(resp) = client.get(&url).send().await else { continue };
        let Ok(v) = resp.json::<Value>().await else { continue };
        let errno = v["errno"].as_i64().unwrap_or(0);
        if errno == 0 {
            let list = v["list"].as_array().or_else(|| v["data"]["list"].as_array());
            if let Some(list) = list {
                return Ok(list.iter().map(pan_api::parse_item).collect());
            }
        } else {
            last_err = match errno {
                105 => "分享链接已失效或被删除".into(),
                -9 => "提取码校验未通过".into(),
                e => format!("获取分享文件列表失败 [errno {e}]"),
            };
        }
    }
    Err(if last_err.is_empty() {
        "获取分享文件列表失败".into()
    } else {
        last_err
    })
}

/// 解析分享链接：返回 shareid / uk / sekey 与文件列表。
/// 私密分享且未提供提取码时返回 need_pwd=true。
#[tauri::command]
pub async fn share_parse(link: String, pwd: String) -> Result<ShareInfo, String> {
    let (surl, pwd_from_link) = extract_surl(&link)
        .ok_or("无法识别分享链接，请粘贴 pan.baidu.com 的分享链接（可整段粘贴含提取码的文本）")?;
    let pwd = if pwd.trim().is_empty() {
        pwd_from_link
    } else {
        pwd.trim().to_string()
    };

    let jar = std::sync::Arc::new(reqwest::cookie::Jar::default());
    let client = reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .cookie_provider(jar.clone())
        .build()
        .map_err(|e| e.to_string())?;

    let mut html = fetch_share_page(&client, &surl).await?;
    let (mut shareid, mut uk) = (extract_field(&html, "shareid"), extract_field(&html, "uk"));
    let mut sekey = String::new();

    // 页面拿不到 shareid —— 私密分享，先校验提取码
    if shareid.is_empty() || uk.is_empty() {
        if pwd.is_empty() {
            return Ok(ShareInfo {
                need_pwd: true,
                ..Default::default()
            });
        }
        let code = surl.strip_prefix('1').unwrap_or(&surl).to_string();
        sekey = verify_password(&client, &code, &pwd).await?;
        // 显式写入 BDCLND cookie，避免 Set-Cookie Path 限制
        if !sekey.is_empty() {
            if let Ok(u) = reqwest::Url::parse("https://pan.baidu.com") {
                jar.add_cookie_str(&format!("BDCLND={sekey}"), &u);
            }
        }
        html = fetch_share_page(&client, &surl).await?;
        shareid = extract_field(&html, "shareid");
        uk = extract_field(&html, "uk");
    }
    if shareid.is_empty() || uk.is_empty() {
        return Err("解析分享页面失败：链接可能已失效，或页面结构发生变化".into());
    }

    let files = match share_list(&client, &shareid, &uk).await {
        Ok(f) if !f.is_empty() => f,
        ok_or_err => extract_files_from_html(&html).ok_or(match ok_or_err {
            Err(e) => e,
            _ => "分享内容为空".to_string(),
        })?,
    };

    Ok(ShareInfo {
        need_pwd: false,
        shareid,
        uk,
        sekey,
        files,
    })
}

/// 将分享文件转存到自己网盘（保存到「来自：分享者」目录）
#[tauri::command]
pub async fn share_transfer(
    shareid: String,
    uk: String,
    sekey: String,
    fs_ids: Vec<u64>,
) -> Result<Vec<TransferResult>, String> {
    if fs_ids.is_empty() {
        return Err("请先选择要转存的文件".into());
    }
    let token = oauth::ensure_token().await?;
    let filelist = serde_json::to_string(
        &fs_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
    )
    .unwrap_or_default();

    let mut url = format!(
        "https://pan.baidu.com/rest/2.0/xpan/share?method=transfer&access_token={token}&shareid={shareid}&from={uk}"
    );
    if !sekey.is_empty() {
        // randsk 本身已是 URL 编码态；裸值则补编码
        let s = if sekey.contains('%') {
            sekey
        } else {
            pan_api::urlencode(&sekey)
        };
        url.push_str("&sekey=");
        url.push_str(&s);
    }

    let v: Value = reqwest::Client::new()
        .post(&url)
        .header("User-Agent", pan_api::PAN_UA)
        .form(&[("filelist", filelist.as_str())])
        .send()
        .await
        .map_err(|e| format!("转存请求失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("转存响应解析失败: {e}"))?;

    match v["errno"].as_i64().unwrap_or(0) {
        0 => {}
        12 | 4 => return Err("转存失败：单次转存文件数或免费额度超出限制".into()),
        105 => return Err("转存失败：文件名非法或目录层级超限".into()),
        130 => return Err("转存失败：网盘剩余空间不足".into()),
        -33 => return Err("转存失败：转存文件数超限".into()),
        e => return Err(format!("转存失败 [errno {e}]")),
    }

    let mut out = Vec::new();
    if let Some(list) = v["extra"]["list"].as_array() {
        for it in list {
            out.push(TransferResult {
                from_fs_id: it["from_fs_id"].as_u64().unwrap_or(0),
                fs_id: it["to_fs_id"].as_u64().unwrap_or(0),
                path: it["to"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    if out.is_empty() {
        return Err("转存结果解析失败，请到网盘「来自：分享」目录查看".into());
    }
    Ok(out)
}

/// 读取本地图片为 base64（供前端 jsQR 解码二维码）
#[tauri::command]
pub fn read_image_base64(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取图片失败: {e}"))?;
    Ok(STANDARD.encode(bytes))
}
