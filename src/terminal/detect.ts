import type { AgentName } from "../types";

/**
 * 输入行追踪器：从 xterm.onData 的原始按键流中重建"当前输入行"。
 * 用户按 Enter 时返回完整行，供命令检测使用。
 *
 * 局限（设计如此，属可接受的启发式）：
 * - ↑/↓ 历史回调、方向键等控制序列出现时，行内容变得不可信 → 清空并放弃当前 chunk
 * - 粘贴（bracketed paste 起始 ESC 序列）会触发同样的放弃逻辑，
 *   但粘贴内容本身是纯文本 chunk，其中若含 \r 仍会被逐行捕获
 */
export class LineTracker {
  private buf = "";

  feed(chunk: string): string | null {
    let completed: string | null = null;
    for (const ch of chunk) {
      const code = ch.codePointAt(0) ?? 0;
      if (ch === "\r" || ch === "\n") {
        if (completed === null) completed = this.buf;
        this.buf = "";
        continue;
      }
      if (ch === "\u007f") {
        // Backspace/Del：删掉一个字符
        this.buf = this.buf.slice(0, -1);
        continue;
      }
      if (code < 32) {
        // 其他控制字符（Ctrl+C、方向键转义序列开头等）：行不可信，放弃本 chunk 剩余部分
        this.buf = "";
        return completed;
      }
      this.buf += ch;
    }
    return completed;
  }

  reset(): void {
    this.buf = "";
  }
}

/**
 * 从一行 shell 命令中提取 agent 调用（claude / codex）。
 * 支持：路径前缀（D:\x\claude、./claude）、.exe 后缀、
 *       复合命令（cd foo && claude、claude; echo done）、引号包裹。
 * 排除：--help / -h / --version 这类非交互调用。
 */
export function extractAgent(line: string): AgentName | null {
  const segments = line.split(/&&|\|\||[;|&]/);
  for (const seg of segments) {
    const trimmed = seg.trim();
    if (!trimmed) continue;
    if (/(^|\s)(--help|-h|--version)(\s|$)/.test(trimmed)) continue;

    const m = trimmed.match(/^("(?:[^"]*)"|'([^']*)'|(\S+))/);
    const first = (m?.[1] ?? m?.[2] ?? m?.[3] ?? "").replace(/^["']|["']$/g, "");
    if (!first) continue;

    const base = first.replace(/^.*[\\/]/, "").toLowerCase().replace(/\.exe$/, "");
    if (base === "claude") return "claude";
    if (base === "codex") return "codex";
    if (base === "pi") return "pi";
  }
  return null;
}
