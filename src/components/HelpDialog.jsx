import { useEffect, useState } from "react";
import { api } from "../api";

/** AI 接入帮助：MCP 配置与本地 API 说明 */
export default function HelpDialog({ onClose }) {
  const [port, setPort] = useState(8790);
  // script: { path, created } | null（null 且无错误 = 检查中）
  const [script, setScript] = useState(null);
  const [scriptErr, setScriptErr] = useState("");
  const [copied, setCopied] = useState("");

  useEffect(() => {
    api
      .getApiPort()
      .then((p) => p && setPort(p))
      .catch(() => {});
    api
      .setupMcpScript()
      .then(setScript)
      .catch((e) => setScriptErr(String(e)));
  }, []);

  const base = `http://127.0.0.1:${port}`;
  const mcpPath = script?.path || "C:\\Users\\你的用户名\\baidupan-mcp.js";
  const fetchCmd = `curl.exe ${base}/mcp.js -o "${mcpPath}"`;

  const mcpConfig = JSON.stringify(
    {
      mcpServers: {
        baidupan: {
          command: "node",
          args: [mcpPath],
          env: { BAIDUPAN_API: base },
        },
      },
    },
    null,
    2
  );

  const copy = async (text, tag) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(tag);
      setTimeout(() => setCopied(""), 1500);
    } catch {}
  };

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal help-modal" onClick={(e) => e.stopPropagation()}>
        <h3>AI 接入帮助（MCP / 本地 API）</h3>

        <div className="help-body">
          <p className="help-lead">
            应用运行时会在本机开放 API（端口 {port}，仅 127.0.0.1 可访问）。
            让 AI 助手（Trae、Claude Desktop、Cursor 等）使用你的百度网盘，只需下面两步：
          </p>

          <h4>第 1 步 · MCP 脚本（已自动完成）</h4>
          {scriptErr ? (
            <>
              <div className="help-err">✗ 自动保存失败：{scriptErr}</div>
              <p className="help-note">可手动执行以下命令后重新打开本窗口：</p>
              <div className="help-code" onClick={() => copy(fetchCmd, "cmd")} title="点击复制">
                {fetchCmd}
                <span className="copy-hint">{copied === "cmd" ? "已复制" : "复制"}</span>
              </div>
            </>
          ) : script ? (
            <div className="help-ok" title={script.path}>
              ✓ {script.created ? "已自动保存到" : "已就绪"}：{script.path}
            </div>
          ) : (
            <div className="help-note">正在检查 / 保存脚本…</div>
          )}

          <h4>第 2 步 · 在 AI 客户端的 MCP 配置中加入（路径已自动填好）</h4>
          <div className="help-code" onClick={() => copy(mcpConfig, "cfg")} title="点击复制">
            {mcpConfig}
            <span className="copy-hint">{copied === "cfg" ? "已复制" : "复制"}</span>
          </div>
          <p className="help-note">
            配置后 AI 自动获得工具：pan_health / pan_user / pan_list / pan_search /
            pan_download / pan_download_dir / pan_tasks。对 AI 说“把我网盘里的 xx 下载到桌面”即可。
          </p>

          <h4>不配 MCP 也可以：直接调用 REST API</h4>
          <div className="help-code">{`curl "${base}/files?dir=/"
curl "${base}/search?key=关键词"
curl -X POST ${base}/download -d "{\\"fs_id\\":123,\\"save_path\\":\\"D:/文件.pdf\\"}"
curl ${base}/tasks`}</div>
          <p className="help-note">完整接口说明：GET {base}/</p>

          <p className="help-note">
            注意：API 仅监听本机，登录凭据保存在应用内不会外泄；使用时保持本应用运行并已登录。
          </p>
        </div>

        <div className="modal-ops">
          <button className="btn primary" onClick={onClose}>
            知道了
          </button>
        </div>
      </div>
    </div>
  );
}
