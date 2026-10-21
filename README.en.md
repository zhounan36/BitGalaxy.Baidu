# BaiduPan Lite (BaiduPanLite)

[简体中文](README.md) | English

A lightweight Baidu Netdisk (Baidu Pan) client for Windows, built with Tauri 2. Small installer footprint, low memory usage, full file management and transfer capabilities, plus a built-in local API and MCP server that lets AI assistants (Trae, Claude Desktop, Cursor, etc.) operate your cloud drive directly.

## Features

- **Sign-in**: Baidu OAuth 2.0 authorization with QR-code login in the browser; tokens auto-renew (valid for 30 days)
- **File browsing**: directory navigation, pagination, sorting by name/size/time, recursive search
- **File management**: create folders, rename, delete (multi-select batch operations supported)
- **Upload & download**: file upload; single-file download; recursive download of an entire directory packaged as a zip; real-time transfer progress panel
- **Share transfer**: parse share links (supports extraction codes and QR-code screenshot recognition), transfer files to your drive in one click
- **AI integration**:
  - Local HTTP API (listens on `127.0.0.1` only, default port `8790`)
  - Zero-dependency MCP server script (Node 18+, stdio transport); tokens always stay inside the app and never pass through third parties

## Tech Stack

| Layer | Technology |
| --- | --- |
| Frontend | React 18 + Vite 5 |
| Desktop framework | Tauri 2 (Rust backend) |
| Networking | reqwest (Rust) / fetch (MCP script) |
| Packaging | NSIS installer (size-optimized: opt-level=z + LTO + strip) |

## Project Structure

```
├── src/                  # Frontend (React)
│   ├── components/       # Login, file table, toolbar, transfer panel, share/help dialogs, etc.
│   ├── api.js            # Wrappers for Tauri backend commands
│   └── ...
├── src-tauri/            # Backend (Rust)
│   └── src/
│       ├── oauth.rs      # OAuth authorization & token management
│       ├── pan_api.rs    # Baidu Netdisk OpenAPI wrappers
│       ├── upload.rs     # Upload (chunked)
│       ├── download.rs   # Download & directory zip packaging
│       ├── share.rs      # Share link parsing & transfer
│       ├── api_server.rs # Local HTTP API (127.0.0.1)
│       └── ...
├── server/mcp.js         # MCP server script (bundled in the app, served via /mcp.js)
└── baidupan-mcp.js       # Copy of the MCP script
```

## Prerequisites

- [Node.js](https://nodejs.org/) 18+
- [Rust](https://www.rust-lang.org/) (stable toolchain)
- Windows 10/11 (currently only the NSIS installer is packaged)

## Getting Started

```bash
# Install dependencies
npm install

# Development mode (starts Vite + Tauri window automatically)
npm run tauri dev

# Build for release (produces the NSIS installer)
npm run tauri build
```

## Usage

1. **Sign in**: Launch the app and click sign-in. The Baidu authorization page opens (QR code supported); the app returns automatically once authorized.
2. **File operations**: The toolbar supports upload, new folder, and search; each file row supports rename, delete, and download; select multiple files for batch delete or zip download.
3. **Share transfer**: Click "Transfer Share", paste a share link (with extraction code), or pick a screenshot containing a QR code for automatic recognition, then choose files to transfer.
4. **AI integration**: Click "AI Integration Help". The app automatically saves the MCP script to your user directory (`%USERPROFILE%\baidupan-mcp.js`); paste the configuration into your AI client as instructed in the dialog.

## AI Integration (MCP)

With the app running and signed in, add the following to your AI client's MCP configuration:

```json
{
  "mcpServers": {
    "baidupan": {
      "command": "node",
      "args": ["C:\\Users\\YourName\\baidupan-mcp.js"],
      "env": { "BAIDUPAN_API": "http://127.0.0.1:8790" }
    }
  }
}
```

Available MCP tools:

| Tool | Description |
| --- | --- |
| `pan_health` | Check whether the app is running and signed in |
| `pan_user` | Account info and drive capacity |
| `pan_list` | List directory contents (sorting and pagination supported) |
| `pan_search` | Recursively search file names |
| `pan_download` | Download a single file (async task) |
| `pan_download_dir` | Recursively download a directory as a zip (async task) |
| `pan_tasks` | Query download task progress |

## Local HTTP API

Started automatically with the app (accessible from `127.0.0.1` only):

| Endpoint | Description |
| --- | --- |
| `GET /` | API self-description |
| `GET /health` | App and sign-in status |
| `GET /user` | User info and capacity |
| `GET /files?dir=/&order=name&desc=0&start=0&limit=100` | List directory |
| `GET /search?key=keyword&dir=/` | Recursive search |
| `POST /download` | Download a file: `{"fs_id", "save_path", "name"?}` |
| `POST /download_dir` | Package a directory as zip: `{"dir", "save_path"}` |
| `GET /tasks` | Download task progress |
| `GET /mcp.js` | Download the MCP server script |

Errors are returned uniformly as `{"error": "..."}`; not signed in returns HTTP 401.

## Environment Variables

| Variable | Default | Description |
| --- | --- | --- |
| `BAIDU_API_PORT` | `8790` | Local API port (retries with +1 increments if occupied, up to 10 attempts) |
| `BAIDU_AUTH_MODE` | auto | Force an auth mode (`web` = authorization-code mode / `device` = device-code mode) |

## Security Notes

- The local API listens on `127.0.0.1` only; external devices cannot access it
- The access_token / refresh_token are stored only in the local app config; the MCP script merely forwards requests and never touches tokens
- Release builds use the authorization-server mode; the client does not embed a SecretKey

## Author

zhounan36@163.com
