import { normalizeWindowsPath } from "../util";

/** 按 shell 生成目录切换命令；PowerShell 使用字面量路径，CMD 同时切换盘符。 */
export function changeDirectoryCommand(shellId: string, path: string): string {
  path = normalizeWindowsPath(path);
  if (!path || /[\0\r\n"]/.test(path)) throw new Error("目录路径无效");
  if (shellId === "cmd") {
    // 百分号通过环境变量单次展开为字面量，避免路径中的 %NAME% 被二次解释。
    const literal = path.replace(/%/g, "%STERM_PATH_PERCENT%");
    return (path.startsWith("\\\\") ? "pushd " : "cd /d ") + '"' + literal + '"';
  }
  return "Set-Location -LiteralPath '" + path.replace(/'/g, "''") + "'";
}
