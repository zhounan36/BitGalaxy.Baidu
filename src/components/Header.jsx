import { formatSize } from "../utils";

export default function Header({ user, onRefresh, onLogout, onHelp }) {
  const u = user?.user || {};
  const name = u.netdisk_name || u.baidu_name || "已登录";
  const total = user?.total || 0;
  const used = user?.used || 0;
  const pct = total ? Math.min(100, (used / total) * 100).toFixed(1) : 0;

  return (
    <header className="app-header">
      <div className="brand">
        <svg viewBox="0 0 48 48" width="26" height="26">
          <path
            d="M12 34a8 8 0 0 1-.9-15.95A12 12 0 0 1 34.5 16 7.5 7.5 0 0 1 34 31H12z"
            fill="#2b5cff"
          />
        </svg>
        <span>百度网盘 Lite</span>
      </div>

      <div className="user-area">
        {u.avatar_url && <img className="avatar" src={u.avatar_url} alt="" />}
        <span className="username">{name}</span>
        <div className="quota" title={`${formatSize(used)} / ${formatSize(total)}`}>
          <div className="quota-bar">
            <div className="quota-fill" style={{ width: `${pct}%` }} />
          </div>
          <span className="quota-text">
            {formatSize(used)} / {formatSize(total)}
          </span>
        </div>
        <button className="icon-btn" title="AI 接入帮助" onClick={onHelp}>
          ?
        </button>
        <button className="icon-btn" title="刷新" onClick={onRefresh}>
          ⟳
        </button>
        <button className="icon-btn" title="退出登录" onClick={onLogout}>
          ⏻
        </button>
      </div>
    </header>
  );
}
