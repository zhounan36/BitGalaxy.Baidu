import { formatSize, formatPercent } from "../utils";

const PHASE_TEXT = {
  hashing: "计算校验…",
  uploading: "上传中",
  merging: "合并文件…",
  downloading: "下载中",
  packing: "打包中",
  done: "已完成",
  error: "失败",
};

export default function TransferPanel({ tasks, collapsed, onToggle, onClear }) {
  const list = Object.values(tasks);
  if (list.length === 0) return null;
  const active = list.filter((t) => t.phase !== "done" && t.phase !== "error").length;

  return (
    <div className={"transfer" + (collapsed ? " collapsed" : "")}>
      <div className="transfer-head" onClick={onToggle}>
        <span className="transfer-title">
          传输列表（{active} 个进行中 / 共 {list.length} 个）{collapsed ? " ▲" : " ▼"}
        </span>
        <button
          className="btn ghost sm"
          onClick={(e) => {
            e.stopPropagation();
            onClear();
          }}
        >
          清除已完成
        </button>
      </div>
      {!collapsed && (
        <div className="transfer-list">
          {list.map((t) => (
            <div key={t.id} className={"transfer-row " + t.phase}>
              <span className={"dir-badge " + t.kind}>{t.kind === "upload" ? "↑" : "↓"}</span>
              <span className="t-name" title={t.name}>
                {t.name}
              </span>
              <div className="t-bar">
                <div
                  className={"t-fill " + t.phase}
                  style={{
                    width:
                      t.phase === "done"
                        ? "100%"
                        : t.total
                        ? `${Math.min(100, (t.done / t.total) * 100)}%`
                        : "0%",
                  }}
                />
              </div>
              <span className="t-status">
                {t.phase === "error"
                  ? t.message || "失败"
                  : t.phase === "done"
                  ? t.message || "完成"
                  : `${PHASE_TEXT[t.phase] || ""} ${
                      t.phase === "uploading" || t.phase === "downloading"
                        ? formatPercent(t.done, t.total)
                        : ""
                    } ${t.total ? `(${formatSize(t.done)}/${formatSize(t.total)})` : ""} ${
                      t.phase === "downloading" && t.message ? `· ${t.message}` : ""
                    }`}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
