// Tauri 后端命令封装
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

export const api = {
  getSettings: () => invoke("get_settings"),
  getApiPort: () => invoke("get_api_port"),
  setupMcpScript: () => invoke("setup_mcp_script"),
  startAuth: () => invoke("start_auth"),
  waitAuth: () => invoke("wait_auth"),
  logout: () => invoke("logout"),
  getUser: () => invoke("get_user"),
  listFiles: (dir, order, desc, start, limit) =>
    invoke("list_files", { dir, order, desc, start, limit }),
  searchFiles: (key, dir) => invoke("search_files", { key, dir }),
  deleteFiles: (paths) => invoke("delete_files", { paths }),
  renameFile: (path, newName) => invoke("rename_file", { path, newName }),
  createDir: (path) => invoke("create_dir", { path }),
  uploadFiles: (localPaths, remoteDir) =>
    invoke("upload_files", { localPaths, remoteDir }),
  downloadFile: (fsId, savePath, name) =>
    invoke("download_file", { fsId, savePath, name }),
  pauseDownload: (id) => invoke("pause_download", { id }),
  resumeDownload: (id) => invoke("resume_download", { id }),
  downloadZip: (items, savePath, taskName) =>
    invoke("download_zip", { items, savePath, taskName }),
  listFilesRecursive: (dirs) => invoke("list_files_recursive", { dirs }),
  shareParse: (link, pwd) => invoke("share_parse", { link, pwd }),
  shareTransfer: (shareid, uk, sekey, fsIds) =>
    invoke("share_transfer", { shareid, uk, sekey, fsIds }),
  readImageBase64: (path) => invoke("read_image_base64", { path }),
};

/** 选择本地文件（多选） */
export async function pickUploadFiles() {
  const res = await openDialog({ multiple: true, title: "选择要上传的文件" });
  if (!res) return [];
  return Array.isArray(res) ? res : [res];
}

/** 选择保存位置 */
export async function pickSavePath(defaultName) {
  return saveDialog({ title: "保存文件", defaultPath: defaultName });
}

/** 选择下载保存目录 */
export async function pickDownloadDir() {
  return openDialog({ directory: true, title: "选择保存位置" });
}

/** 选择包含二维码的截图 */
export async function pickQrImage() {
  return openDialog({
    title: "选择包含二维码的截图",
    filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "bmp", "webp"] }],
  });
}

export { openUrl };
