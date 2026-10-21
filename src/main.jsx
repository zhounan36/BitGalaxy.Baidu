import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App.jsx";
import "./styles.css";

// Tauri 的 invoke/listen 只在桌面窗口的 WebView 中可用
const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function BrowserNotice() {
  return (
    <div className="login-wrap">
      <div className="login-card">
        <h1>请通过桌面应用运行</h1>
        <p className="login-sub">
          当前是在普通浏览器中打开页面，无法访问百度网盘的本地能力。
        </p>
        <div className="setup-form">
          <p className="hint">
            开发调试请在项目根目录执行：
            <code style={{ userSelect: "text" }}>npm run tauri dev</code>
            <br />
            （会自动弹出桌面窗口，浏览器地址栏请勿直接访问 5173）
            <br />
            <br />
            或直接运行打包好的程序：
            <code style={{ userSelect: "text" }}>
              src-tauri\target\release\baidupan-lite.exe
            </code>
          </p>
        </div>
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    {isTauri ? <App /> : <BrowserNotice />}
  </React.StrictMode>
);
