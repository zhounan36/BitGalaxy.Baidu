import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, openUrl } from "../api";

export default function Login({ onAuthed }) {
  const [webAuth, setWebAuth] = useState(false);
  const [device, setDevice] = useState(null);
  const [authUrl, setAuthUrl] = useState("");
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const waitingRef = useRef(false);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => setWebAuth(!!s.web_auth))
      .catch(() => {});
  }, []);

  // 网页授权时授权服务器不可达，后端自动回退设备码扫码并推送新授权信息
  useEffect(() => {
    let un;
    listen("auth://fallback", (e) => {
      setAuthUrl("");
      setDevice(e.payload);
      openUrl(e.payload.verification_url).catch(() => {});
    }).then((fn) => (un = fn));
    return () => un && un();
  }, []);

  const copyCode = async () => {
    try {
      await navigator.clipboard.writeText(device.user_code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {}
  };

  const begin = async () => {
    setError("");
    setDevice(null);
    setAuthUrl("");
    try {
      const d = await api.startAuth();
      // 自动打开授权页
      if (d.mode === "web") {
        setAuthUrl(d.url);
        openUrl(d.url).catch(() => {});
      } else {
        setDevice(d);
        openUrl(d.verification_url).catch(() => {});
      }
      if (waitingRef.current) return;
      waitingRef.current = true;
      setWaiting(true);
      api
        .waitAuth()
        .then(() => {
          waitingRef.current = false;
          setWaiting(false);
          onAuthed();
        })
        .catch((e) => {
          waitingRef.current = false;
          setWaiting(false);
          setError(String(e));
        });
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="login-wrap">
      <div className="login-card">
        <div className="login-logo">
          <svg viewBox="0 0 48 48" width="56" height="56">
            <path
              d="M12 34a8 8 0 0 1-.9-15.95A12 12 0 0 1 34.5 16 7.5 7.5 0 0 1 34 31H12z"
              fill="#2b5cff"
            />
          </svg>
        </div>
        <h1>百度网盘 Lite</h1>
        <p className="login-sub">轻量客户端 · 使用官方开放平台接口</p>

        {device ? (
          <div className="auth-box">
            {device.qrcode_url && (
              <img className="qrcode" src={device.qrcode_url} alt="授权二维码" />
            )}
            <p className="auth-tip">请用百度账号扫码，或在打开的页面中输入用户码</p>
            <div className="user-code" onClick={copyCode} title="点击复制">
              {device.user_code}
              <span className="copy-hint">{copied ? "已复制" : "复制"}</span>
            </div>
            <button
              className="btn ghost block"
              onClick={() => openUrl(device.verification_url).catch(() => {})}
            >
              重新打开授权页面
            </button>
            {waiting ? (
              <p className="waiting">
                <span className="spinner" /> 等待授权中…（在浏览器完成登录并点击“授权”）
              </p>
            ) : (
              <button className="btn primary block" onClick={begin}>
                重新获取用户码
              </button>
            )}
          </div>
        ) : authUrl ? (
          <div className="auth-box">
            <p className="auth-tip">
              已在浏览器打开百度授权页，请登录你的百度账号（支持手机百度 APP 扫码）并点击“授权”
            </p>
            <button
              className="btn ghost block"
              onClick={() => openUrl(authUrl).catch(() => {})}
            >
              重新打开授权页面
            </button>
            {waiting ? (
              <p className="waiting">
                <span className="spinner" /> 等待授权中…
              </p>
            ) : (
              <button className="btn primary block" onClick={begin}>
                重新发起授权
              </button>
            )}
          </div>
        ) : (
          <div className="login-actions">
            <button className="btn primary block" onClick={begin}>
              登录百度网盘
            </button>
            <p className="hint">
              {webAuth
                ? "使用你自己的百度账号授权登录，无需任何配置"
                : "使用你自己的百度账号扫码授权，无需任何配置"}
            </p>
          </div>
        )}

        {error && <p className="error-text">{error}</p>}
      </div>
    </div>
  );
}
