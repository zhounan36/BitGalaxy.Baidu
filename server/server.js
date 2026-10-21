// 百度网盘 Lite 授权服务器（方案 C：授权码模式）
// 职责：保管 SecretKey，用授权码换 token、代理 refresh_token 续期。
// 客户端只内置 AppKey + 本服务器地址，绝不包含 SecretKey。
//
// 部署（Node 18+，零依赖）：
//   BAIDU_APP_KEY=xxx BAIDU_SECRET_KEY=xxx PORT=8787 node server.js
// 需在百度网盘开放平台控制台把「授权回调页」注册为：
//   http://你的IP:端口/callback
// 若经反向代理或注册的回调地址与实际访问地址不一致，需再设置：
//   REDIRECT_URI=http://注册的完整回调地址
//
// 接口：
//   GET  /callback?code=&state=   百度授权后的浏览器回调（换 token 并暂存）
//   GET  /result?state=           客户端轮询取回 token（一次性）
//   POST /refresh  {refresh_token} 客户端代理续期

import http from "node:http";

const PORT = process.env.PORT || 8787;
const APP_KEY = process.env.BAIDU_APP_KEY || "";
const SECRET_KEY = process.env.BAIDU_SECRET_KEY || "";
const REDIRECT_URI = process.env.REDIRECT_URI || "";
const TOKEN_URL = "https://openapi.baidu.com/oauth/2.0/token";

// state -> { ok, payload }，暂存 120 秒，取一次即焚
const results = new Map();

if (!APP_KEY || !SECRET_KEY) {
  console.error("缺少环境变量 BAIDU_APP_KEY / BAIDU_SECRET_KEY，无法启动");
  process.exit(1);
}

function send(res, status, body, type = "application/json; charset=utf-8") {
  res.writeHead(status, { "Content-Type": type });
  res.end(body);
}

function pickTokens(v) {
  return {
    access_token: v.access_token,
    refresh_token: v.refresh_token,
    expires_in: v.expires_in || 2592000,
  };
}

async function exchangeToken(params) {
  const r = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(params).toString(),
  });
  return r.json();
}

function resultPage(title) {
  return `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title>
<style>body{font-family:system-ui,-apple-system,"Segoe UI",sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f5f6fa;color:#333}
.card{background:#fff;padding:40px 56px;border-radius:16px;box-shadow:0 6px 24px rgba(0,0,0,.08);text-align:center}
h1{font-size:20px;margin:0 0 10px}p{margin:0;color:#666}</style></head>
<body><div class="card"><h1>${title}</h1><p>现在可以关闭此页面，回到「百度网盘 Lite」应用</p></div></body></html>`;
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host || "localhost"}`);
  try {
    // 百度授权后的浏览器回调
    if (url.pathname === "/callback") {
      const code = url.searchParams.get("code");
      const state = url.searchParams.get("state");
      if (!code || !state) {
        return send(res, 400, "缺少 code / state", "text/plain; charset=utf-8");
      }
      // 换 token 时 redirect_uri 必须与授权请求里的一致
      const redirectUri =
        REDIRECT_URI || `http://${req.headers.host}/callback`;
      const v = await exchangeToken({
        grant_type: "authorization_code",
        code,
        client_id: APP_KEY,
        client_secret: SECRET_KEY,
        redirect_uri: redirectUri,
      });
      if (v.access_token) {
        results.set(state, { ok: true, payload: pickTokens(v) });
        setTimeout(() => results.delete(state), 120_000);
        console.log(`[callback] state=${state} 授权成功`);
        return send(res, 200, resultPage("授权成功"), "text/html; charset=utf-8");
      }
      const msg = v.error_description || v.error || "授权失败";
      results.set(state, { ok: false, payload: msg });
      setTimeout(() => results.delete(state), 120_000);
      console.warn(`[callback] state=${state} 失败: ${msg}`);
      return send(res, 200, resultPage("授权失败：" + msg), "text/html; charset=utf-8");
    }

    // 客户端轮询取回 token
    if (url.pathname === "/result") {
      const state = url.searchParams.get("state");
      if (!state) return send(res, 400, JSON.stringify({ status: "error", message: "缺少 state" }));
      const item = results.get(state);
      if (!item) return send(res, 200, JSON.stringify({ status: "pending" }));
      results.delete(state); // 一次性
      if (item.ok) return send(res, 200, JSON.stringify({ status: "ok", ...item.payload }));
      return send(res, 200, JSON.stringify({ status: "error", message: item.payload }));
    }

    // 客户端代理续期
    if (url.pathname === "/refresh" && req.method === "POST") {
      let body = "";
      for await (const chunk of req) body += chunk;
      const rt = (JSON.parse(body || "{}")).refresh_token;
      if (!rt) return send(res, 400, JSON.stringify({ message: "缺少 refresh_token" }));
      const v = await exchangeToken({
        grant_type: "refresh_token",
        refresh_token: rt,
        client_id: APP_KEY,
        client_secret: SECRET_KEY,
      });
      if (v.access_token) return send(res, 200, JSON.stringify(pickTokens(v)));
      return send(res, 400, JSON.stringify({
        message: v.error_description || v.error || "刷新失败",
      }));
    }

    if (url.pathname === "/") {
      return send(res, 200, "baidupan-lite auth server", "text/plain; charset=utf-8");
    }
    send(res, 404, "not found", "text/plain; charset=utf-8");
  } catch (e) {
    console.error("[error]", e);
    send(res, 500, JSON.stringify({ message: String(e) }));
  }
});

server.listen(PORT, () => {
  console.log(`baidupan-lite 授权服务器已启动: http://0.0.0.0:${PORT}`);
  console.log(`请确认开放平台控制台的「授权回调页」已注册为 ${
    REDIRECT_URI || `http://<你的IP>:${PORT}/callback`
  }`);
});
