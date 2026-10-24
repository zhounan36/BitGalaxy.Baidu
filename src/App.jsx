import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, pickUploadFiles, pickSavePath, pickDownloadDir } from "./api";
import Login from "./components/Login";
import Header from "./components/Header";
import Toolbar from "./components/Toolbar";
import FileTable from "./components/FileTable";
import TransferPanel from "./components/TransferPanel";
import Modal from "./components/Modal";
import Toast from "./components/Toast";
import ShareDialog from "./components/ShareDialog";
import HelpDialog from "./components/HelpDialog";

const PAGE = 100;

export default function App() {
  const [ready, setReady] = useState(false);
  const [logged, setLogged] = useState(false);
  const [user, setUser] = useState(null); // { user, total, used }
  const [dir, setDir] = useState("/");
  const [items, setItems] = useState([]);
  const [loading, setLoading] = useState(false);
  const [order, setOrder] = useState({ key: "name", desc: false });
  const [hasMore, setHasMore] = useState(false);
  const [selected, setSelected] = useState(() => new Set());
  const [tasks, setTasks] = useState({}); // id -> progress
  const [showTransfers, setShowTransfers] = useState(true);
  const [toast, setToast] = useState(null);
  const [modal, setModal] = useState(null);
  const [searchMode, setSearchMode] = useState("");
  const [shareOpen, setShareOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const orderRef = useRef(order);
  orderRef.current = order;

  const notify = useCallback((message, type = "info") => {
    setToast({ message: String(message), type, ts: Date.now() });
  }, []);

  // toast 自动消失
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 3500);
    return () => clearTimeout(t);
  }, [toast]);

  // 初始化：读取配置 -> 若已登录拉取用户信息（顺便校验 token）
  useEffect(() => {
    (async () => {
      try {
        const s = await api.getSettings();
        if (s.logged_in) {
          const u = await api.getUser();
          setUser(u);
          setLogged(true);
        }
      } catch (e) {
        /* token 失效则停留在登录页 */
      }
      setReady(true);
    })();
  }, []);

  // 传输进度事件
  useEffect(() => {
    let un;
    listen("transfer://progress", (e) => {
      setTasks((prev) => ({ ...prev, [e.payload.id]: e.payload }));
    }).then((fn) => (un = fn));
    return () => un && un();
  }, []);

  const fetchList = useCallback(
    async (d, o, start, replace) => {
      setLoading(true);
      try {
        const list = await api.listFiles(d, o.key, o.desc, start, PAGE);
        setItems((prev) => (replace ? list : [...prev, ...list]));
        setHasMore(list.length >= PAGE);
      } catch (e) {
        notify(e, "error");
      }
      setLoading(false);
    },
    [notify]
  );

  // 主界面加载/目录变化时拉取
  useEffect(() => {
    if (logged && !searchMode) fetchList(dir, orderRef.current, 0, true);
  }, [logged, dir]); // eslint-disable-line react-hooks/exhaustive-deps

  const goTo = (d) => {
    if (searchMode) setSearchMode("");
    setSelected(new Set());
    setDir(d);
  };

  const changeOrder = (key) => {
    const o = order.key === key ? { key, desc: !order.desc } : { key, desc: false };
    setOrder(o);
    if (!searchMode) fetchList(dir, o, 0, true);
  };

  const loadMore = () => fetchList(dir, orderRef.current, items.length, false);

  const refresh = () => {
    if (searchMode) doSearch(searchMode);
    else fetchList(dir, orderRef.current, 0, true);
  };

  const doSearch = async (key) => {
    if (!key.trim()) {
      setSearchMode("");
      fetchList(dir, orderRef.current, 0, true);
      return;
    }
    setLoading(true);
    try {
      const list = await api.searchFiles(key.trim(), dir);
      setItems(list);
      setHasMore(false);
      setSearchMode(key.trim());
    } catch (e) {
      notify(e, "error");
    }
    setLoading(false);
  };

  // ---------- 操作 ----------

  const onUpload = async () => {
    const paths = await pickUploadFiles();
    if (!paths.length) return;
    setShowTransfers(true);
    try {
      await api.uploadFiles(paths, dir);
      notify("上传完成");
      refresh();
    } catch (e) {
      notify(`上传失败: ${e}`, "error");
    }
  };

  const onDownload = async (item) => {
    const savePath = await pickSavePath(item.name);
    if (!savePath) return;
    setShowTransfers(true);
    try {
      await api.downloadFile(item.fs_id, savePath, item.name);
      notify(`已下载: ${item.name}`);
    } catch (e) {
      notify(`下载失败: ${e}`, "error");
    }
  };

  // 展开选中项：文件直接用；目录递归列出内部文件，并计算本地/包内相对路径
  const expandSelected = async (sel) => {
    const files = sel.filter((s) => !s.is_dir).map((s) => ({ ...s, rel: s.name }));
    for (const d of sel.filter((s) => s.is_dir)) {
      const inner = await api.listFilesRecursive([d.path]);
      for (const f of inner) {
        files.push({ ...f, rel: f.path.slice(d.path.length).replace(/^\//, "") });
      }
    }
    return files;
  };

  // 批量下载：目录递归展开，保持目录结构，逐个下载到所选目录
  const onDownloadSelected = async () => {
    const sel = items.filter((i) => selected.has(i.fs_id));
    if (!sel.length) return notify("请先选择要下载的项", "error");
    let files;
    try {
      files = await expandSelected(sel);
    } catch (e) {
      return notify(`展开目录失败: ${e}`, "error");
    }
    if (!files.length) return notify("选中的内容里没有文件", "error");
    const dirPath = await pickDownloadDir();
    if (!dirPath) return;
    const base = dirPath.replace(/[\\/]+$/, "");
    setShowTransfers(true);
    setSelected(new Set());
    notify(`开始批量下载 ${files.length} 个文件`);
    (async () => {
      for (const f of files) {
        try {
          await api.downloadFile(f.fs_id, `${base}/${f.rel}`, f.name);
        } catch (e) {
          notify(`下载失败 ${f.name}: ${e}`, "error");
        }
      }
    })();
  };

  // 打包下载：目录递归展开后，后端下载到临时目录并合成一个 zip
  const onZipSelected = async () => {
    const sel = items.filter((i) => selected.has(i.fs_id));
    if (!sel.length) return notify("请先选择要打包的项", "error");
    let files;
    try {
      files = await expandSelected(sel);
    } catch (e) {
      return notify(`展开目录失败: ${e}`, "error");
    }
    if (!files.length) return notify("选中的内容里没有文件", "error");
    const savePath = await pickSavePath(`baidupan-${files.length}files.zip`);
    if (!savePath) return;
    setShowTransfers(true);
    setSelected(new Set());
    const name = `打包下载（${files.length} 个文件）`;
    try {
      await api.downloadZip(
        files.map((f) => ({ fs_id: f.fs_id, rel_path: f.rel, size: f.size })),
        savePath,
        name
      );
      notify("打包下载完成");
    } catch (e) {
      notify(`打包下载失败: ${e}`, "error");
    }
  };

  // 分享转存成功后：选目录并依次下载转存后的文件（目录只转存不下载）
  const handleTransferred = async (picked, results) => {
    setShareOpen(false);
    setShowTransfers(true);
    refresh();
    const byFrom = new Map(results.map((r) => [r.from_fs_id, r]));
    const files = picked.filter((f) => !f.is_dir);
    const dirs = picked.length - files.length;
    if (dirs > 0) notify(`已转存 ${dirs} 个文件夹到网盘，可在「来自：分享」目录中查看`);
    const downloadable = files
      .map((f) => ({ f, r: byFrom.get(f.fs_id) }))
      .filter((x) => x.r && x.r.fs_id);
    const missing = files.length - downloadable.length;
    if (missing > 0) notify(`${missing} 个文件请到网盘「来自：分享」目录手动下载`);
    if (!downloadable.length) return;
    const dirPath = await pickDownloadDir();
    if (!dirPath) {
      notify("已转存，可稍后在网盘列表中下载");
      return;
    }
    const base = dirPath.replace(/[\\/]+$/, "");
    (async () => {
      for (const { f, r } of downloadable) {
        try {
          await api.downloadFile(r.fs_id, `${base}/${f.name}`, f.name);
        } catch (e) {
          notify(`下载失败 ${f.name}: ${e}`, "error");
        }
      }
    })();
  };

  const askDelete = (item) => {
    const paths = item
      ? [item.path]
      : [...selected]
          .map((id) => items.find((i) => i.fs_id === id)?.path)
          .filter(Boolean);
    if (!paths.length) return notify("请先选择要删除的项", "error");
    setModal({
      type: "confirm",
      title: "删除确认",
      text: `确定删除 ${paths.length} 项？删除后可在网盘「回收站」中找回。`,
      onOk: async () => {
        try {
          await api.deleteFiles(paths);
          notify("已删除");
          setSelected(new Set());
          refresh();
        } catch (e) {
          notify(e, "error");
        }
      },
    });
  };

  const askRename = (item) => {
    setModal({
      type: "prompt",
      title: "重命名",
      initial: item.name,
      onOk: async (v) => {
        try {
          await api.renameFile(item.path, v);
          refresh();
        } catch (e) {
          notify(e, "error");
        }
      },
    });
  };

  const askMkdir = () => {
    setModal({
      type: "prompt",
      title: "新建文件夹",
      initial: "",
      onOk: async (v) => {
        const p = (dir === "/" ? "" : dir) + "/" + v.trim();
        try {
          await api.createDir(p);
          refresh();
        } catch (e) {
          notify(e, "error");
        }
      },
    });
  };

  const onLogout = async () => {
    try {
      await api.logout();
    } catch {}
    setLogged(false);
    setUser(null);
    setItems([]);
    setSelected(new Set());
    setTasks({});
    setDir("/");
    setSearchMode("");
  };

  const toggleSel = (id) => {
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const allSelected = items.length > 0 && items.every((i) => selected.has(i.fs_id));
  const toggleAll = () => {
    setSelected(allSelected ? new Set() : new Set(items.map((i) => i.fs_id)));
  };

  const clearFinishedTasks = () => {
    setTasks((prev) => {
      const next = {};
      for (const [id, t] of Object.entries(prev)) {
        if (t.phase !== "done" && t.phase !== "error") next[id] = t;
      }
      return next;
    });
  };

  // 暂停 / 继续下载任务（后端保留 .part，继续时从断点续传）
  const pauseDownload = async (id) => {
    try {
      await api.pauseDownload(id);
    } catch (e) {
      notify(e, "error");
    }
  };

  const resumeDownload = async (id) => {
    try {
      await api.resumeDownload(id);
    } catch (e) {
      notify(e, "error");
    }
  };

  if (!ready) return <div className="boot">正在启动…</div>;
  if (!logged)
    return (
      <Login
        onAuthed={async () => {
          try {
            const u = await api.getUser();
            setUser(u);
            setLogged(true);
          } catch (e) {
            notify(e, "error");
          }
        }}
      />
    );

  return (
    <div className="app">
      <Header
        user={user}
        onRefresh={refresh}
        onLogout={onLogout}
        onHelp={() => setHelpOpen(true)}
      />
      <Toolbar
        dir={dir}
        searchMode={searchMode}
        onNav={goTo}
        onSearch={doSearch}
        onUpload={onUpload}
        onMkdir={askMkdir}
        onDeleteSelected={() => askDelete(null)}
        onShareDownload={() => setShareOpen(true)}
        onDownloadSelected={onDownloadSelected}
        onZipSelected={onZipSelected}
        selectedCount={selected.size}
      />
      <main className="content">
        <FileTable
          items={items}
          loading={loading}
          order={order}
          onSort={changeOrder}
          selected={selected}
          allSelected={allSelected}
          onToggleAll={toggleAll}
          onToggle={toggleSel}
          onOpen={(item) => (item.is_dir ? goTo(item.path) : null)}
          onDownload={onDownload}
          onRename={askRename}
          onDelete={askDelete}
          hasMore={hasMore}
          onLoadMore={loadMore}
          searchMode={searchMode}
        />
      </main>
      <TransferPanel
        tasks={tasks}
        collapsed={!showTransfers}
        onToggle={() => setShowTransfers((v) => !v)}
        onClear={clearFinishedTasks}
        onPause={pauseDownload}
        onResume={resumeDownload}
      />
      {modal && <Modal modal={modal} onClose={() => setModal(null)} />}
      {helpOpen && <HelpDialog onClose={() => setHelpOpen(false)} />}
      {shareOpen && (
        <ShareDialog onClose={() => setShareOpen(false)} onTransferred={handleTransferred} />
      )}
      {toast && <Toast toast={toast} />}
    </div>
  );
}
