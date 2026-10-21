import { useState } from "react";

export default function Toolbar({
  dir,
  searchMode,
  onNav,
  onSearch,
  onUpload,
  onMkdir,
  onDeleteSelected,
  onShareDownload,
  onDownloadSelected,
  onZipSelected,
  selectedCount,
}) {
  const [key, setKey] = useState("");

  const crumbs = ["全部文件", ...(dir === "/" ? [] : dir.slice(1).split("/"))];
  const paths = (() => {
    let acc = "";
    return ["/", ...dir.slice(1).split("/").filter(Boolean).map((p) => (acc += "/" + p))];
  })();

  return (
    <div className="toolbar">
      <div className="crumbs">
        {crumbs.map((c, i) => (
          <span key={i} className="crumb-item">
            {i > 0 && <span className="sep">/</span>}
            <a
              href="#"
              onClick={(e) => {
                e.preventDefault();
                onNav(paths[i]);
              }}
            >
              {c}
            </a>
          </span>
        ))}
        {searchMode && <span className="search-tag">搜索“{searchMode}”的结果</span>}
      </div>

      <div className="toolbar-actions">
        <input
          className="search-input"
          placeholder="搜索当前目录（含子目录）"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSearch(key);
          }}
        />
        <button className="btn primary sm" onClick={onUpload}>
          ↑ 上传
        </button>
        <button className="btn ghost sm" onClick={onMkdir}>
          ＋ 新建文件夹
        </button>
        <button className="btn ghost sm" onClick={onShareDownload}>
          ⇩ 分享下载
        </button>
        {selectedCount > 0 && (
          <>
            <button className="btn ghost sm" onClick={onDownloadSelected}>
              ↓ 批量下载（{selectedCount}）
            </button>
            <button className="btn ghost sm" onClick={onZipSelected}>
              ▣ 打包下载
            </button>
            <button className="btn danger sm" onClick={onDeleteSelected}>
              删除（{selectedCount}）
            </button>
          </>
        )}
      </div>
    </div>
  );
}
