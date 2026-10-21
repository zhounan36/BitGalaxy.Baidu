import FileIcon from "./FileIcon";
import { formatSize, formatTime } from "../utils";

const SORT_LABEL = { name: "名称", size: "大小", time: "修改时间" };

export default function FileTable({
  items,
  loading,
  order,
  onSort,
  selected,
  allSelected,
  onToggleAll,
  onToggle,
  onOpen,
  onDownload,
  onRename,
  onDelete,
  hasMore,
  onLoadMore,
  searchMode,
}) {
  return (
    <div className="table-wrap">
      <div className="thead">
        <div className="th col-check">
          <input type="checkbox" checked={allSelected} onChange={onToggleAll} />
        </div>
        <div className="th col-name sortable" onClick={() => onSort("name")}>
          文件名 <span className="arrow">{arrow(order, "name")}</span>
        </div>
        <div className="th col-size sortable" onClick={() => onSort("size")}>
          大小 <span className="arrow">{arrow(order, "size")}</span>
        </div>
        <div className="th col-time sortable" onClick={() => onSort("time")}>
          修改时间 <span className="arrow">{arrow(order, "time")}</span>
        </div>
        <div className="th col-ops">操作</div>
      </div>

      <div className="tbody">
        {loading && items.length === 0 && (
          <div className="row-empty">
            <span className="spinner" /> 加载中…
          </div>
        )}
        {!loading && items.length === 0 && (
          <div className="row-empty">
            {searchMode ? "没有找到匹配的文件" : "此目录为空"}
          </div>
        )}
        {items.map((it) => (
          <div
            key={it.fs_id}
            className={"row" + (selected.has(it.fs_id) ? " selected" : "")}
            onDoubleClick={() => onOpen(it)}
          >
            <div className="td col-check">
              <input
                type="checkbox"
                checked={selected.has(it.fs_id)}
                onChange={() => onToggle(it.fs_id)}
              />
            </div>
            <div className="td col-name">
              <FileIcon item={it} />
              <span
                className={"name" + (it.is_dir ? " dir" : "")}
                onClick={() => it.is_dir && onOpen(it)}
                title={it.path}
              >
                {it.name}
              </span>
            </div>
            <div className="td col-size">{it.is_dir ? "-" : formatSize(it.size)}</div>
            <div className="td col-time">{formatTime(it.mtime)}</div>
            <div className="td col-ops">
              {!it.is_dir && (
                <button className="op" title="下载" onClick={() => onDownload(it)}>
                  ↓
                </button>
              )}
              <button className="op" title="重命名" onClick={() => onRename(it)}>
                ✎
              </button>
              <button className="op del" title="删除" onClick={() => onDelete(it)}>
                ✕
              </button>
            </div>
          </div>
        ))}
        {hasMore && !loading && (
          <div className="load-more">
            <button className="btn ghost sm" onClick={onLoadMore}>
              加载更多
            </button>
          </div>
        )}
        {loading && items.length > 0 && (
          <div className="load-more muted">加载中…</div>
        )}
      </div>
    </div>
  );
}

function arrow(order, key) {
  if (order.key !== key) return "";
  return order.desc ? "▼" : "▲";
}
