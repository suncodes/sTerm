/** base64 → Uint8Array（PTY 输出经 base64 编码跨过 IPC） */
export function b64ToU8(s: string): Uint8Array {
  const bin = atob(s);
  const u = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
  return u;
}

/** 相对时间显示 */
export function relTime(ms: number): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  const d = new Date(ms);
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** 统一 Windows 分隔符，保留盘符根目录。 */
export function normalizeWindowsPath(path: string): string {
  const windows = path.replace(/\//g, "\\");
  const trimmed = windows.replace(/\\+$/, "");
  if (!trimmed) return windows ? "\\" : "";
  return /^[A-Za-z]:$/.test(trimmed) && windows.endsWith("\\") ? trimmed + "\\" : trimmed;
}

/** 路径比较用：统一分隔符、去尾分隔符并忽略大小写。 */
export function normPath(path: string): string {
  return normalizeWindowsPath(path).toLowerCase();
}

export function isPathWithin(path: string, directory: string): boolean {
  const current = normPath(path);
  const parent = normPath(directory);
  return current === parent || current.startsWith(parent.endsWith("\\") ? parent : parent + "\\");
}

/** 支持本机盘符及 UNC 的 OSC 7 file URI；兼容 prompt 输出的未编码路径。 */
export function cwdFromFileUri(data: string): string | null {
  if (!data.startsWith("file://")) return null;
  let path = data.slice(7);
  if (path.startsWith("localhost/")) path = path.slice(9);
  if (/^\/+[A-Za-z]:\//.test(path)) path = path.replace(/^\/+/, "");
  else if (path.startsWith("//")) path = "\\\\" + path.replace(/^\/+/, "");
  else if (!path.startsWith("/") && !/^[A-Za-z]:\//.test(path)) path = "\\\\" + path;
  try { path = decodeURIComponent(path); } catch { /* 保留未编码的 % */ }
  path = normalizeWindowsPath(path);
  return /^[A-Za-z]:\\/.test(path) || /^\\\\[^\\]+\\[^\\]+/.test(path) ? path : null;
}
