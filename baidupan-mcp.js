#!/usr/bin/env node
// 百度网盘 Lite MCP 服务器：把应用的本地 REST API 包装成 MCP 工具，供 AI 客户端调用。
// 零依赖（Node 18+），stdio 传输。token 始终保存在应用内，本脚本只做转发。
//
// 前提：百度网盘 Lite 应用已运行（本地 API 默认 http://127.0.0.1:8790）且已登录。
//
// AI 客户端配置示例（Trae / Claude Desktop / Cursor 等）：
//   {
//     "mcpServers": {
//       "baidupan": {
//         "command": "node",
//         "args": ["E:\\CODE2\\BitGalaxy.Baidu\\server\\mcp.js"],
//         "env": { "BAIDUPAN_API": "http://127.0.0.1:8790" }
//       }
//     }
//   }
// env 可省略；端口非默认时（应用启动时设置过 BAIDU_API_PORT）才需要指定。

import readline from "node:readline";

const API = process.env.BAIDUPAN_API || "http://127.0.0.1:8790";

const tools = [
  {
    name: "pan_health",
    description:
      "检查百度网盘 Lite 应用是否在运行、账号是否已登录。调用其他工具前建议先检查。",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "pan_user",
    description: "获取当前登录的百度账号信息与网盘容量（总空间/已用空间）。",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "pan_list",
    description: "列出网盘指定目录下的文件和子目录。",
    inputSchema: {
      type: "object",
      properties: {
        dir: { type: "string", description: "网盘目录路径，默认根目录 /" },
        order: {
          type: "string",
          description: "排序字段：name / size / time，默认 name",
          enum: ["name", "size", "time"],
        },
        desc: { type: "boolean", description: "是否倒序，默认 false" },
        start: { type: "integer", description: "分页起始，默认 0" },
        limit: { type: "integer", description: "每页数量（1-1000），默认 100" },
      },
    },
  },
  {
    name: "pan_search",
    description: "在网盘中递归搜索文件名包含关键词的文件。",
    inputSchema: {
      type: "object",
      properties: {
        key: { type: "string", description: "搜索关键词（必填）" },
        dir: { type: "string", description: "限定搜索的目录，默认根目录 /" },
      },
      required: ["key"],
    },
  },
  {
    name: "pan_download",
    description:
      "下载网盘文件到本机（异步任务，立即返回任务 id，用 pan_tasks 查询进度，phase=done 即完成）。",
    inputSchema: {
      type: "object",
      properties: {
        fs_id: {
          type: "integer",
          description: "文件 id（必填，从 pan_list / pan_search 结果获取）",
        },
        save_path: {
          type: "string",
          description: "本地完整保存路径（必填，含文件名），父目录会自动创建",
        },
        name: { type: "string", description: "显示名（可省略，默认取 save_path 文件名）" },
      },
      required: ["fs_id", "save_path"],
    },
  },
  {
    name: "pan_download_dir",
    description:
      "把整个网盘目录递归下载并打包为一个 zip（异步任务，用 pan_tasks 查询进度）。",
    inputSchema: {
      type: "object",
      properties: {
        dir: { type: "string", description: "网盘目录路径（必填）" },
        save_path: {
          type: "string",
          description: "本地 zip 完整保存路径（必填，含 .zip 文件名）",
        },
      },
      required: ["dir", "save_path"],
    },
  },
  {
    name: "pan_tasks",
    description:
      "查询下载任务列表与进度。phase 含义：starting/downloading/packing/done/error。",
    inputSchema: { type: "object", properties: {} },
  },
];

async function callApi(method, path, body) {
  const res = await fetch(API + path, {
    method,
    headers: body ? { "Content-Type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = text;
  }
  if (!res.ok) {
    const msg =
      data && typeof data === "object" && data.error ? data.error : text;
    const hint =
      res.status === 401 ? "（应用内未登录，请先在百度网盘 Lite 中登录）" : "";
    throw new Error(`HTTP ${res.status}: ${msg}${hint}`);
  }
  return data;
}

const handlers = {
  pan_health: () => callApi("GET", "/health"),
  pan_user: () => callApi("GET", "/user"),
  pan_list: (a) =>
    callApi(
      "GET",
      `/files?dir=${encodeURIComponent(a.dir || "/")}` +
        `&order=${a.order || "name"}&desc=${a.desc ? 1 : 0}` +
        `&start=${a.start || 0}&limit=${a.limit || 100}`
    ),
  pan_search: (a) =>
    callApi(
      "GET",
      `/search?key=${encodeURIComponent(a.key || "")}&dir=${encodeURIComponent(a.dir || "/")}`
    ),
  pan_download: (a) => callApi("POST", "/download", a),
  pan_download_dir: (a) => callApi("POST", "/download_dir", a),
  pan_tasks: () => callApi("GET", "/tasks"),
};

// ---------- MCP stdio（JSON-RPC 2.0，按行分隔） ----------

function send(msg) {
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function reply(id, result) {
  send({ jsonrpc: "2.0", id, result });
}
function replyError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  line = line.trim();
  if (!line) return;
  let req;
  try {
    req = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = req;
  const isNotification = id === undefined || id === null;

  Promise.resolve()
    .then(async () => {
      switch (method) {
        case "initialize":
          return reply(id, {
            protocolVersion: params?.protocolVersion || "2024-11-05",
            capabilities: { tools: {} },
            serverInfo: { name: "baidupan-lite", version: "1.0.0" },
          });
        case "ping":
          return reply(id, {});
        case "tools/list":
          return reply(id, { tools });
        case "tools/call": {
          const name = params?.name;
          const fn = handlers[name];
          if (!fn) {
            return reply(id, {
              content: [{ type: "text", text: `未知工具: ${name}` }],
              isError: true,
            });
          }
          try {
            const result = await fn(params?.arguments || {});
            return reply(id, {
              content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
            });
          } catch (e) {
            const hint = /fetch failed|ECONNREFUSED/i.test(String(e.message))
              ? "（无法连接本地 API，请确认百度网盘 Lite 应用已运行）"
              : "";
            return reply(id, {
              content: [{ type: "text", text: `调用失败: ${e.message}${hint}` }],
              isError: true,
            });
          }
        }
        default:
          // 通知消息（如 notifications/initialized）不回应；未知请求返回错误
          if (!isNotification) {
            replyError(id, -32601, `未知方法: ${method}`);
          }
      }
    })
    .catch((e) => {
      if (!isNotification) replyError(id, -32603, String(e));
    });
});
