# 百度网盘 Lite（BaiduPanLite）

简体中文 | [English](README.en.md)

轻量版百度网盘 Windows 客户端，基于 Tauri 2 构建。安装包小、内存占用低，具备完整的文件管理与传输能力，并内置本地 API 与 MCP 服务器，可让 AI 助手（Trae、Claude Desktop、Cursor 等）直接操作你的网盘。

## 功能特性

- **账号登录**：百度 OAuth 2.0 授权，支持浏览器扫码登录，token 自动续期（30 天有效）
- **文件浏览**：目录导航、分页加载、按名称/大小/时间排序、递归搜索
- **文件管理**：新建文件夹、重命名、删除（支持多选批量操作）
- **上传下载**：文件上传；单文件下载；整个目录递归下载并自动打包 zip；实时传输进度面板
- **分享转存**：解析分享链接（支持提取码、识别分享二维码截图），一键转存到我的网盘
- **AI 接入**：
  - 本地 HTTP API（仅监听 `127.0.0.1`，默认端口 `8790`）
  - 零依赖 MCP 服务器脚本（Node 18+，stdio 传输），token 始终保存在应用内，不经过第三方

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | React 18 + Vite 5 |
| 桌面框架 | Tauri 2（Rust 后端） |
| 网络请求 | reqwest（Rust）/ fetch（MCP 脚本） |
| 打包 | NSIS 安装包（体积优化：opt-level=z + LTO + strip） |

## 目录结构

```
├── src/                  # 前端（React）
│   ├── components/       # 登录、文件列表、工具栏、传输面板、分享/帮助弹窗等
│   ├── api.js            # Tauri 后端命令封装
│   └── ...
├── src-tauri/            # 后端（Rust）
│   └── src/
│       ├── oauth.rs      # OAuth 授权与 token 管理
│       ├── pan_api.rs    # 百度网盘 OpenAPI 封装
│       ├── upload.rs     # 上传（分片）
│       ├── download.rs   # 下载与目录打包 zip
│       ├── share.rs      # 分享解析与转存
│       ├── api_server.rs # 本地 HTTP API（127.0.0.1）
│       └── ...
├── server/mcp.js         # MCP 服务器脚本（内置于应用，可经 /mcp.js 下发）
└── baidupan-mcp.js       # MCP 脚本副本
```

## 环境要求

- [Node.js](https://nodejs.org/) 18+
- [Rust](https://www.rust-lang.org/)（stable 工具链）
- Windows 10/11（当前仅打包 NSIS 安装包）

## 快速开始

```bash
# 安装依赖
npm install

# 开发模式（自动启动 Vite + Tauri 窗口）
npm run tauri dev

# 打包发布版（生成 NSIS 安装包）
npm run tauri build
```

## 使用说明

1. **登录**：启动应用后点击登录，会打开百度授权页（支持扫码），授权完成后自动返回。
2. **文件操作**：工具栏支持上传、新建文件夹、搜索；文件行支持重命名、删除、下载；勾选多个文件可批量删除或打包下载。
3. **分享转存**：点击「转存分享」粘贴分享链接（含提取码），或选择含二维码的截图自动识别，挑选文件后转存。
4. **AI 接入**：点击「AI 接入帮助」，应用会自动把 MCP 脚本保存到用户目录（`%USERPROFILE%\baidupan-mcp.js`），按弹窗提示把配置粘贴进 AI 客户端即可。

## AI 接入（MCP）

应用运行且已登录后，在 AI 客户端的 MCP 配置中加入：

```json
{
  "mcpServers": {
    "baidupan": {
      "command": "node",
      "args": ["C:\\Users\\你的用户名\\baidupan-mcp.js"],
      "env": { "BAIDUPAN_API": "http://127.0.0.1:8790" }
    }
  }
}
```

可用 MCP 工具：

| 工具 | 说明 |
| --- | --- |
| `pan_health` | 检查应用运行与登录状态 |
| `pan_user` | 账号信息与网盘容量 |
| `pan_list` | 列出目录内容（支持排序、分页） |
| `pan_search` | 递归搜索文件名 |
| `pan_download` | 下载单个文件（异步任务） |
| `pan_download_dir` | 整目录递归下载打包 zip（异步任务） |
| `pan_tasks` | 查询下载任务进度 |

## 本地 HTTP API

应用启动后自动在本机开放（仅 `127.0.0.1` 可访问）：

| 端点 | 说明 |
| --- | --- |
| `GET /` | 接口自描述 |
| `GET /health` | 应用与登录状态 |
| `GET /user` | 用户信息与容量 |
| `GET /files?dir=/&order=name&desc=0&start=0&limit=100` | 列目录 |
| `GET /search?key=关键词&dir=/` | 递归搜索 |
| `POST /download` | 下载单文件：`{"fs_id", "save_path", "name"?}` |
| `POST /download_dir` | 整目录打包 zip：`{"dir", "save_path"}` |
| `GET /tasks` | 下载任务进度 |
| `GET /mcp.js` | 下载 MCP 服务器脚本 |

错误统一返回 `{"error": "..."}`；未登录返回 HTTP 401。

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `BAIDU_API_PORT` | `8790` | 本地 API 端口（被占用时依次 +1 重试，最多 10 个） |
| `BAIDU_AUTH_MODE` | 自动 | 强制指定授权模式（`web` 授权码模式 / `device` 设备码模式） |

## 安全说明

- 本地 API 仅监听 `127.0.0.1`，外部设备无法访问
- access_token / refresh_token 仅保存在本机应用配置中，MCP 脚本只做请求转发，不接触 token
- 分发构建采用授权服务器模式，客户端不内置 SecretKey

## 作者

zhounan36@163.com
