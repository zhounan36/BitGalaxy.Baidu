import { useEffect, useRef, useState } from "react";
import jsQR from "jsqr";
import { api, pickQrImage } from "../api";
import { formatSize } from "../utils";

const LINK_RE =
  /https?:\/\/[^\s]*(?:pan|yun)\.baidu\.com\/(?:s\/[\w-]+|share\/init\?surl=[\w-]+)/i;
const PWD_IN_URL_RE = /[?&]pwd=([0-9a-zA-Z]{4})/i;
const PWD_TEXT_RE = /(?:提取码|访问码|密码)[:：\s]*([0-9a-zA-Z]{4})/i;

/** 从粘贴文本 / 二维码内容中解析分享链接与提取码 */
export function parseShareText(text) {
  const m = text.match(LINK_RE);
  const bare = text.trim().match(/^[0-9A-Za-z_-]{8,}$/);
  const link = m ? m[0] : bare ? text.trim() : "";
  const p = link.match(PWD_IN_URL_RE) || text.match(PWD_TEXT_RE);
  return { link, pwd: p ? p[1] : "" };
}

/** 读取本地截图并解码二维码，返回二维码文本内容 */
async function decodeQrFromFile(path) {
  const b64 = await api.readImageBase64(path);
  const ext = (path.split(".").pop() || "").toLowerCase();
  const mime = { png: "png", jpg: "jpeg", jpeg: "jpeg", bmp: "bmp", webp: "webp" }[ext] || "png";
  const img = await new Promise((resolve, reject) => {
    const i = new Image();
    i.onload = () => resolve(i);
    i.onerror = () => reject(new Error("截图加载失败"));
    i.src = `data:image/${mime};base64,${b64}`;
  });
  // 大图等比缩小，避免解码过慢
  const max = 1800;
  const scale = Math.min(1, max / Math.max(img.width, img.height));
  const w = Math.max(1, Math.round(img.width * scale));
  const h = Math.max(1, Math.round(img.height * scale));
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  ctx.drawImage(img, 0, 0, w, h);
  const data = ctx.getImageData(0, 0, w, h);
  const code = jsQR(data.data, w, h, { inversionAttempts: "attemptBoth" });
  return code ? code.data : null;
}

export default function ShareDialog({ onClose, onTransferred }) {
  const [link, setLink] = useState("");
  const [pwd, setPwd] = useState("");
  const [info, setInfo] = useState(null);
  const [checked, setChecked] = useState(() => new Set());
  const [busy, setBusy] = useState(""); // "" | parse | qr | transfer
  const [error, setError] = useState("");
  const pwdRef = useRef(null);

  useEffect(() => {
    if (info && info.files.length) setChecked(new Set(info.files.map((f) => f.fs_id)));
  }, [info]);

  const doParse = async (linkArg, pwdArg) => {
    const raw = (linkArg ?? link).trim();
    if (!raw) return setError("请先粘贴分享链接");
    const parsed = parseShareText(raw);
    const usePwd = ((pwdArg ?? pwd).trim() || parsed.pwd).trim();
    setError("");
    setBusy("parse");
    try {
      const res = await api.shareParse(raw, usePwd);
      if (res.need_pwd) {
        setError("该分享需要提取码，请填写后重试");
        pwdRef.current?.focus();
        return;
      }
      setInfo(res);
      if (!res.files.length) setError("分享内容为空");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const scanQr = async () => {
    const p = await pickQrImage();
    if (!p) return;
    setError("");
    setBusy("qr");
    try {
      const text = await decodeQrFromFile(p);
      if (!text) return setError("未在截图中识别到二维码");
      const { link: l, pwd: pw } = parseShareText(text);
      if (!l) return setError("二维码内容不是百度网盘分享链接");
      setLink(l);
      if (pw) setPwd(pw);
      await doParse(text, pw);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const toggle = (id) => {
    setChecked((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const allChecked =
    info && info.files.length > 0 && info.files.every((f) => checked.has(f.fs_id));
  const toggleAll = () =>
    setChecked(allChecked ? new Set() : new Set(info.files.map((f) => f.fs_id)));

  const totalChecked = info
    ? info.files.filter((f) => checked.has(f.fs_id)).reduce((s, f) => s + (f.is_dir ? 0 : f.size), 0)
    : 0;

  const doTransfer = async () => {
    if (!info) return;
    const picked = info.files.filter((f) => checked.has(f.fs_id));
    if (!picked.length) return setError("请先勾选要下载的内容");
    setError("");
    setBusy("transfer");
    try {
      const results = await api.shareTransfer(
        info.shareid,
        info.uk,
        info.sekey,
        picked.map((f) => f.fs_id)
      );
      onTransferred(picked, results);
    } catch (e) {
      setError(String(e));
      setBusy("");
    }
  };

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal share-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>分享下载</h3>

        <div className="share-form">
          <input
            className="share-link"
            placeholder="粘贴分享链接（支持整段带提取码的文本）"
            value={link}
            onChange={(e) => setLink(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && doParse(link, pwd)}
          />
          <input
            className="share-pwd"
            ref={pwdRef}
            placeholder="提取码"
            maxLength={4}
            value={pwd}
            onChange={(e) => setPwd(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && doParse(link, pwd)}
          />
        </div>

        <div className="share-actions">
          <button className="btn ghost sm" disabled={!!busy} onClick={scanQr}>
            {busy === "qr" ? <span className="spinner" /> : "▣"} 扫二维码截图
          </button>
          <button className="btn primary sm" disabled={!!busy} onClick={() => doParse(link, pwd)}>
            {busy === "parse" ? <span className="spinner" /> : "解析分享"}
          </button>
        </div>

        {error && <div className="share-error">{error}</div>}

        {info && info.files.length > 0 && (
          <>
            <div className="share-files">
              {info.files.map((f) => (
                <label key={f.fs_id} className="share-file">
                  <input
                    type="checkbox"
                    checked={checked.has(f.fs_id)}
                    onChange={() => toggle(f.fs_id)}
                  />
                  <span className="fname" title={f.path}>
                    {f.is_dir ? "📁 " : "📄 "}
                    {f.name}
                  </span>
                  <span className="fsize">{f.is_dir ? "文件夹" : formatSize(f.size)}</span>
                </label>
              ))}
            </div>
            <div className="share-bar">
              <label className="share-all">
                <input type="checkbox" checked={allChecked} onChange={toggleAll} />
                全选（已选 {checked.size}/{info.files.length} · {formatSize(totalChecked)}）
              </label>
              <div className="share-ops">
                <button
                  className="btn ghost sm"
                  onClick={() => {
                    setInfo(null);
                    setChecked(new Set());
                    setError("");
                  }}
                >
                  返回
                </button>
                <button className="btn primary sm" disabled={!!busy} onClick={doTransfer}>
                  {busy === "transfer" ? <span className="spinner" /> : "⇩"} 转存并下载
                </button>
              </div>
            </div>
          </>
        )}

        <p className="share-tip">
          支持公开/私密分享：私密分享需提取码；文件先转存到网盘「来自：分享」目录，再自动下载到本地。
        </p>
      </div>
    </div>
  );
}
