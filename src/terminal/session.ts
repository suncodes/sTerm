import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { LineTracker, extractAgent } from "./detect";
import { AgentState } from "./agentstate";
import { AgentOscChannel, parseAgentKind, PROGRAM_STATUS_RESPONSE } from "./agentevents";
import { MouseModeFilter } from "./mousefilter";
import { changeDirectoryCommand } from "./commands";
import { cwdFromFileUri } from "../util";
import { showToast } from "../ui/toast";
import type { AgentName, ShellInfo } from "../types";

/** 状态栏：当前活动终端尺寸 */
function updateStatusSize(cols: number, rows: number): void {
  const el = document.getElementById("status-size");
  if (el) el.textContent = `${cols} × ${rows}`;
}

export interface SessionOptions {
  id: number;
  /** 终端面板容器（session 会创建自己的 pane 挂进去） */
  panesEl: HTMLElement;
  shell: ShellInfo;
  /** 初始工作目录（null 时由 Rust 侧回退到用户主目录） */
  cwd: string | null;
  /** 终端字号（来自设置） */
  fontSize?: number;
  /** 标签标题（保存工作区或手工重命名时更新，不随目录变化）：不传时用“未命名” */
  title?: string;
  /** shell integration 报告的 cwd 变化（每次 prompt 前触发） */
  onCwd?: (session: TerminalSession, cwd: string) => void;
  onAgent: (session: TerminalSession, agent: AgentName) => void;
  /** Agent 状态变化（running/idle/exited），用于标签状态点 */
  onAgentStatus?: (session: TerminalSession) => void;
  onSidebarToggle?: () => void;
  onNewTab?: () => void;
  onCloseTab?: () => void;
  onSearchOpen?: () => void;
  /** 右键"保存到工作区" */
  onSaveToWorkspace?: (session: TerminalSession) => void;
}

/**
 * 单个终端会话：自包含 pane（xterm + 退出覆盖层）。
 * 输出/退出事件由 TabManager 统一分发到 handleOutput / handleExit。
 */
export class TerminalSession {
  readonly id: number;
  readonly shell: ShellInfo;
  readonly term: Terminal;
  readonly searchAddon = new SearchAddon();

  private pane: HTMLElement;
  /** PTY resize 防抖定时器与上次已下发的尺寸 */
  private ptyResizeTimer: number | null = null;
  private ptySentCols = 0;
  private ptySentRows = 0;
  private resizeObserver: ResizeObserver;
  private resizeInFlight = false;
  private resizePending = false;
  private generation = 0;
  private exitedGeneration = 0;
  private starting = false;
  private disposed = false;
  private termEl: HTMLElement;
  private overlay: HTMLElement;
  private fitAddon = new FitAddon();
  private tracker = new LineTracker();
  private opts: SessionOptions;
  private outQueue: Uint8Array[] = [];
  private mouseModeFilter = new MouseModeFilter();
  private outputGeneration = 0;
  private queuedOutputGeneration = 0;
  private agentOsc = new AgentOscChannel(({ agent, event }) => this.agentState.markAgentEvent(agent, event));
  private flushTimer: number | null = null;
  private alive = false;
  private cwdReported = false;
  private cwdQuery: Promise<void> | null = null;
  activeFlag = false;

  /** 标签标题：保存工作区或手工重命名时更新，不随目录变化。 */
  title: string;
  /** 当前真实工作目录（OSC 7 / PEB 报告） */
  cwd: string | null = null;

  constructor(opts: SessionOptions) {
    this.opts = opts;
    this.id = opts.id;
    this.shell = opts.shell;
    this.cwd = opts.cwd;
    this.title = opts.title ?? "未命名";

    // ---- pane DOM：term-pane > [term 容器, overlay] ----
    this.pane = document.createElement("div");
    this.pane.className = "term-pane";
    this.termEl = document.createElement("div");
    this.termEl.className = "term-inner";
    this.pane.appendChild(this.termEl);

    this.overlay = document.createElement("div");
    this.overlay.className = "term-overlay hidden";
    const title = document.createElement("div");
    title.className = "overlay-title";
    title.textContent = "进程已退出";
    const btn = document.createElement("button");
    btn.textContent = "重新启动 shell";
    btn.onclick = () => void this.restart();
    this.overlay.append(title, btn);
    this.pane.appendChild(this.overlay);
    opts.panesEl.appendChild(this.pane);

    // ---- xterm ----
    this.term = new Terminal({
      fontFamily: '"Maple Mono Normal NL NF CN", "Cascadia Mono", Consolas, monospace',
      fontSize: opts.fontSize ?? 13,
      cursorBlink: true,
      cursorStyle: "bar",
      scrollback: 10000,
      theme: {
        // Win11 PowerShell（Windows Terminal 默认 Campbell 配色）
        background: "#000000",
        foreground: "#CCCCCC",
        cursor: "#FFFFFF",
        cursorAccent: "#0C0C0C",
        selectionBackground: "#FFFFFF",
        selectionForeground: "#000000",
        black: "#0C0C0C",
        red: "#C50F1F",
        green: "#13A10E",
        yellow: "#C19C00",
        blue: "#0037DA",
        magenta: "#881798",
        cyan: "#3A96DD",
        white: "#CCCCCC",
        brightBlack: "#767676",
        brightRed: "#E74856",
        brightGreen: "#16C60C",
        brightYellow: "#F9F1A5",
        brightBlue: "#3B78FF",
        brightMagenta: "#B4009E",
        brightCyan: "#61D6D6",
        brightWhite: "#F2F2F2",
      },
    });
    this.term.loadAddon(this.fitAddon);
    this.term.loadAddon(this.searchAddon);
    this.term.loadAddon(new WebLinksAddon());
    this.term.open(this.termEl);

    // 自定义右键菜单（禁用 WebView2 默认菜单）
    this.pane.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      this.showContextMenu(e);
    });
    this.term.attachCustomKeyEventHandler((ev) => this.handleKey(ev));

    // 输入：先喂给检测器，再原样转发给 PTY
    this.term.onData((data) => {
      const line = this.tracker.feed(data);
      if (line !== null) {
        const agent = extractAgent(line);
        if (agent) {
          this.opts.onAgent(this, agent);
          this.agentState.markAgent(agent);
        }
      }
      void this.write(data);
    });

    // 滚轮由前端处理：备用屏幕不转为方向键，普通屏幕沿用终端回滚。
    this.term.attachCustomWheelEventHandler((e) => {
      if (e.ctrlKey) {
        e.preventDefault();
        e.stopPropagation();
        if (e.deltaY !== 0) {
          this.setFontSize(this.term.options.fontSize! + (e.deltaY < 0 ? 1 : -1));
        }
        return false;
      }
      if (this.term.buffer.active.type === "alternate") {
        e.preventDefault();
        e.stopPropagation();
        return false;
      }
      return true;
    });

    this.resizeObserver = new ResizeObserver(() => this.refit());
    this.resizeObserver.observe(this.termEl);

    // shell integration：prompt 前的 OSC 7 报告真实 cwd
    this.term.parser.registerOscHandler(7, (data) => this.handleOscCwd(data));
    // Codex / Claude 由 TUI 直接报告，支持本地、WSL 与 SSH。
    this.term.parser.registerOscHandler(7501, (data) => {
      if (!this.acceptsOutput()) return true;
      if (data === "?") {
        void this.write(PROGRAM_STATUS_RESPONSE);
        return true;
      }
      return this.agentOsc.acceptProgramStatus(data);
    });
    // 直接消费所在终端的状态消息，WSL/SSH 与本地输出共用同一条 PTY 链路。
    this.term.parser.registerOscHandler(777, (data) => {
      if (!this.acceptsOutput()) return data.startsWith("sterm;");
      return this.agentOsc.accept(data);
    });

    // 终端尺寸变化 → 状态栏
    this.term.onResize(() => {
      if (this.activeFlag) updateStatusSize(this.term.cols, this.term.rows);
    });
  }

  async start(): Promise<void> {
    if (this.disposed || this.starting) return;
    this.starting = true;
    this.alive = false;
    const generation = ++this.generation;
    this.cwdReported = false;
    this.cwdQuery = null;
    this.agentState.reset();
    this.agentOsc.reset();
    this.cancelResize();
    this.ptySentCols = 0;
    this.ptySentRows = 0;
    this.outQueue = [];
    this.mouseModeFilter.reset();
    this.overlay.classList.add("hidden");
    try {
      // 标签已激活，先测量一次；下一帧再次确认布局后再创建 PTY。
      this.fitTerminal();
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      if (this.disposed) return;
      this.fitTerminal();
      const cols = this.term.cols;
      const rows = this.term.rows;
      await invoke("pty_spawn", {
        id: this.id,
        generation,
        shellId: this.shell.id,
        cwd: this.cwd ?? this.opts.cwd,
        cols,
        rows,
      });
      if (this.disposed) {
        await invoke("pty_kill", { id: this.id, generation }).catch(() => {});
        return;
      }
      // shell 可能在 spawn 返回前退出，不能覆盖退出事件设置的状态。
      if (this.exitedGeneration !== generation) {
        this.ptySentCols = cols;
        this.ptySentRows = rows;
        this.alive = true;
        this.refit();
        if (this.activeFlag) this.term.focus();
      }
    } catch (error) {
      if (!this.disposed) {
        this.term.write("\r\nShell 启动失败：" + String(error) + "\r\n");
        this.overlay.classList.remove("hidden");
      }
      console.error("PTY 启动失败", error);
    } finally {
      this.starting = false;
    }
  }

  async restart(): Promise<void> {
    this.tracker.reset();
    await this.start();
  }

  /** Agent 状态机：标签状态点（运行中/空闲/退出） */
  readonly agentState = new AgentState(() => this.opts.onAgentStatus?.(this));

  async write(data: string): Promise<void> {
    if (!this.alive) return;
    await invoke("pty_write", { id: this.id, generation: this.generation, data }).catch(() => {});
  }

  /** 往终端注入一条命令并回车（点击工作区 cd、一键 resume 等） */
  async writeCommand(cmd: string): Promise<void> {
    if (!this.alive) return;
    await this.write(cmd + "\r");
    this.term.focus();
  }

  async changeDirectory(path: string): Promise<void> {
    await this.writeCommand(changeDirectoryCommand(this.shell.id, path));
  }

  /** 已有 OSC 7 时使用上报值；其余查询合并，并丢弃旧 Shell 的响应。 */
  async refreshCwd(): Promise<void> {
    if (this.disposed || !this.alive || this.cwdReported) return;
    if (this.cwdQuery) return this.cwdQuery;
    const generation = this.generation;
    const query: Promise<void> = invoke<string | null>("get_shell_cwd", { id: this.id, generation })
      .then((cwd) => {
        if (!cwd || this.disposed || !this.alive || this.cwdReported || generation !== this.generation) return;
        if (this.cwd !== cwd) {
          this.cwd = cwd;
          this.opts.onCwd?.(this, cwd);
        }
      })
      .catch(() => {})
      .finally(() => { if (this.cwdQuery === query) this.cwdQuery = null; });
    this.cwdQuery = query;
    return query;
  }

  /* ---------- TabManager 分发的事件 ---------- */

  handleOutput(generation: number, u: Uint8Array): void {
    if (!this.disposed && generation === this.generation) this.enqueue(u);
  }

  /** 事件信箱分发来的 Pi 扩展事件。 */
  deliverAgentEvent(generation: number, kind: string): void {
    if (this.disposed || generation !== this.generation || this.exitedGeneration === generation
      || this.agentOsc.hasControl) return;
    const event = parseAgentKind(kind);
    if (event) this.agentState.markAgentEvent(event.agent, event.event);
  }

  handleExit(generation: number): void {
    if (this.disposed || generation !== this.generation) return;
    this.exitedGeneration = generation;
    this.agentState.markExited();
    this.alive = false;
    this.cancelResize();
    this.flush();
    const remaining = this.mouseModeFilter.finish();
    if (remaining) this.writeOutput(remaining);
    this.term.write("\r\n\x1b[90m■ shell 进程已退出\x1b[0m\r\n");
    this.overlay.classList.remove("hidden");
  }

  /* ---------- 激活状态 ---------- */

  /** OSC 7: file:///D:/path → 解析为 Windows 路径并上报 */
  private handleOscCwd(data: string): boolean {
    if (!this.acceptsOutput()) return true;
    if (!data.startsWith("file://")) return false;
    this.agentOsc.shellPrompt();
    this.agentState.markShellPrompt();
    const path = cwdFromFileUri(data);
    if (!path) return true; // 远端 POSIX 路径只清理状态，不当作 Windows 工作区。
    this.cwdReported = true;
    this.cwd = path;
    this.opts.onCwd?.(this, path);
    return true;
  }

  setActive(active: boolean): void {
    this.activeFlag = active;
    this.pane.classList.toggle("active", active);
    if (!active) this.cancelResize();
    if (active) {
      this.refit();
      this.term.focus();
      updateStatusSize(this.term.cols, this.term.rows);
    }
  }

  get isActive(): boolean {
    return this.activeFlag;
  }

  /* ---------- 输出合并：把高频数据攒成 8ms 一批 ---------- */

  private enqueue(u: Uint8Array): void {
    this.outQueue.push(u);
    // 最小化时不叠加输出合并定时器，状态序列及时进入 xterm 的解析队列。
    if (document.visibilityState === "hidden") {
      if (this.flushTimer !== null) window.clearTimeout(this.flushTimer);
      this.flush();
      return;
    }
    if (this.flushTimer === null) {
      this.flushTimer = window.setTimeout(() => this.flush(), 8);
    }
  }

  private flush(): void {
    this.flushTimer = null;
    const q = this.outQueue;
    this.outQueue = [];
    if (q.length === 0) return;
    let total = 0;
    for (const x of q) total += x.length;
    const out = new Uint8Array(total);
    let o = 0;
    for (const x of q) {
      out.set(x, o);
      o += x.length;
    }
    const text = this.mouseModeFilter.filter(out);
    if (text) this.writeOutput(text);
  }

  private acceptsOutput(): boolean {
    return !this.disposed && this.outputGeneration === this.generation
      && this.exitedGeneration !== this.generation;
  }

  private writeOutput(text: string): void {
    const generation = this.generation;
    if (this.queuedOutputGeneration !== generation) {
      this.queuedOutputGeneration = generation;
      // CAN 结束上一代未完成的控制序列；标记与输出在 xterm 队列中保持顺序。
      this.term.write("\x18", () => { this.outputGeneration = generation; });
    }
    this.term.write(text);
  }

  /* ---------- 尺寸 ---------- */

  refit(): void {
    if (this.disposed || !this.activeFlag) return;
    this.cancelResize();
    // 前后端在同一批稳定尺寸上更新，避免拖动过程中反复重排。
    this.ptyResizeTimer = window.setTimeout(() => {
      this.ptyResizeTimer = null;
      void this.syncSize();
    }, 150);
  }

  private cancelResize(): void {
    if (this.ptyResizeTimer !== null) window.clearTimeout(this.ptyResizeTimer);
    this.ptyResizeTimer = null;
    this.resizePending = false;
  }

  private fitTerminal(): { cols: number; rows: number } | null {
    if (this.disposed || !this.activeFlag) return null;
    const rect = this.termEl.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;
    try {
      const size = this.fitAddon.proposeDimensions();
      if (!size || !Number.isInteger(size.cols) || !Number.isInteger(size.rows)
        || size.cols < 2 || size.rows < 1 || size.cols > 32767 || size.rows > 32767) {
        return null;
      }
      this.term.resize(size.cols, size.rows);
      updateStatusSize(size.cols, size.rows);
      return size;
    } catch {
      return null; // 字体或布局尚未就绪，等待后续观察器通知
    }
  }

  private async syncSize(): Promise<void> {
    if (this.disposed || !this.activeFlag) return;
    if (this.resizeInFlight) {
      this.resizePending = true;
      return;
    }
    const size = this.fitTerminal();
    if (!size || !this.alive
      || this.ptySentCols === size.cols && this.ptySentRows === size.rows) return;
    const generation = this.generation;
    this.resizeInFlight = true;
    try {
      await invoke("pty_resize", { id: this.id, generation, ...size });
      if (!this.disposed && generation === this.generation) {
        this.ptySentCols = size.cols;
        this.ptySentRows = size.rows;
      }
    } catch (error) {
      // 保留上次成功的尺寸，下次布局通知仍可重试。
      console.warn("PTY 尺寸同步失败", error);
    } finally {
      this.resizeInFlight = false;
      if (this.resizePending) {
        this.resizePending = false;
        this.refit();
      }
    }
  }

  /* ---------- 快捷键 ---------- */

  private handleKey(ev: KeyboardEvent): boolean {
    if (ev.type !== "keydown") return true;
    const key = ev.key.toLowerCase();
    const ctrl = ev.ctrlKey || ev.metaKey;
    const consume = () => {
      ev.preventDefault();
      ev.stopPropagation();
      return false;
    };

    if (ctrl && !ev.shiftKey && key === "f") {
      this.opts.onSearchOpen?.();
      return consume();
    }
    if (ctrl && !ev.shiftKey && key === "b") {
      this.opts.onSidebarToggle?.();
      return consume();
    }
    if (ctrl && !ev.shiftKey && key === "t") {
      this.opts.onNewTab?.();
      return consume();
    }
    if (ctrl && !ev.shiftKey && key === "w") {
      this.opts.onCloseTab?.();
      return consume();
    }
    if (ctrl && ev.shiftKey && key === "c") {
      void this.copySelection();
      return consume();
    }
    if (ctrl && key === "v") {
      void this.paste();
      return consume();
    }
    if (ctrl && !ev.shiftKey && key === "c") {
      // 智能复制：有选区则复制，否则放行 ^C 中断信号
      if (this.term.hasSelection()) {
        void this.copySelection();
        return consume();
      }
      return true;
    }
    if (ctrl && (key === "=" || key === "+")) {
      this.setFontSize(this.term.options.fontSize! + 1);
      return consume();
    }
    if (ctrl && key === "-") {
      this.setFontSize(this.term.options.fontSize! - 1);
      return consume();
    }
    if (ctrl && key === "0") {
      this.setFontSize(13);
      return consume();
    }
    return true;
  }

  async copySelection(sel = this.term.getSelection()): Promise<void> {
    if (!sel) return;
    try {
      await writeText(sel);
      if (!this.disposed && this.term.getSelection() === sel) this.term.clearSelection();
    } catch (error) {
      console.error("复制失败", error);
      showToast("复制失败，请重试", [], 4000);
    } finally {
      if (!this.disposed && this.activeFlag) this.term.focus();
    }
  }

  private async paste(plain = false): Promise<void> {
    try {
      let text = await readText();
      if (!text || this.disposed) return;
      if (plain) {
        // 粘贴为纯文本：清洗智能引号 / 不换行空格 / 零宽字符，统一换行符。
        text = text
          .replace(/[\u201C\u201D\u201E\u2033]/g, '"')
          .replace(/[\u2018\u2019\u201B\u2032]/g, "'")
          .replace(/[\u00A0\u2007\u202F\u205F]/g, " ")
          .replace(/[\u200B-\u200D\uFEFF]/g, "")
          .replace(/\r\n?/g, "\n");
      }
      if (text) this.term.paste(text);
    } catch (error) {
      console.error("粘贴失败", error);
      showToast("粘贴失败，请重试", [], 4000);
    } finally {
      if (!this.disposed && this.activeFlag) this.term.focus();
    }
  }

  /** 终端右键菜单（Win11 Fluent 质感）：图标 + 快捷键标注 */
  private showContextMenu(e: MouseEvent): void {
    e.preventDefault();
    document.querySelectorAll(".context-menu").forEach((m) => m.remove());

    const menu = document.createElement("div");
    menu.className = "context-menu";
    menu.addEventListener("mousedown", (event) => event.preventDefault());
    const sel = this.term.hasSelection() ? this.term.getSelection() : "";

    const svg = (paths: string): string =>
      `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
    const ICONS = {
      copy: svg(
        '<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>'
      ),
      paste: svg(
        '<rect width="8" height="4" x="8" y="2" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><path d="M12 11h4"/><path d="M12 16h4"/>'
      ),
      selectAll: svg(
        '<path d="M18 8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2h3"/><rect x="8" y="2" width="8" height="4" rx="1"/><path d="M9 14h6"/><path d="M9 18h4"/>'
      ),
      search: svg('<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>'),
      plain: svg(
        '<polyline points="4 7 4 4 20 4 20 7"/><line x1="9" x2="15" y1="20" y2="20"/><line x1="12" x2="12" y1="4" y2="20"/>'
      ),
      saveWs: svg(
        '<path d="M12 10v6"/><path d="m9 13 3 3 3-3"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 0-1.69.9L9.6 9.9H4a2 2 0 0 0-2 2V18a2 2 0 0 0 2 2Z"/>'
      ),
    };

    const mk = (
      label: string,
      icon: string,
      keyHint: string | null,
      enabled: boolean,
      fn: () => void
    ): void => {
      const item = document.createElement("div");
      item.className = "menu-item" + (enabled ? "" : " disabled");
      const iconEl = document.createElement("span");
      iconEl.className = "mi-icon";
      iconEl.innerHTML = icon;
      const labelEl = document.createElement("span");
      labelEl.textContent = label;
      item.appendChild(iconEl);
      item.appendChild(labelEl);
      if (keyHint) {
        const keyEl = document.createElement("span");
        keyEl.className = "mi-key";
        keyEl.textContent = keyHint;
        item.appendChild(keyEl);
      }
      if (enabled) {
        item.onclick = () => {
          fn();
          menu.remove();
        };
      }
      menu.appendChild(item);
    };

    mk("复制", ICONS.copy, "Ctrl+Shift+C", !!sel, () => void this.copySelection(sel));
    mk("粘贴", ICONS.paste, "Ctrl+V", true, () => void this.paste());
    mk("粘贴为纯文本", ICONS.plain, null, true, () => void this.paste(true));
    mk("全选", ICONS.selectAll, null, true, () => this.term.selectAll());
    mk("查找", ICONS.search, "Ctrl+F", true, () => this.opts.onSearchOpen?.());

    // 分隔线 + 工作区/截图
    const sep = document.createElement("div");
    sep.className = "menu-sep";
    menu.appendChild(sep);
    mk("保存到工作区", ICONS.saveWs, null, true, () => this.opts.onSaveToWorkspace?.(this));

    document.body.appendChild(menu);
    const w = menu.offsetWidth || 200;
    const h = menu.offsetHeight || 150;
    menu.style.left = `${Math.max(8, Math.min(e.clientX, window.innerWidth - w - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(e.clientY, window.innerHeight - h - 8))}px`;
  }

  private setFontSize(size: number): void {
    const s = Math.max(8, Math.min(32, size));
    this.term.options.fontSize = s;
    this.refit();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.alive = false;
    this.cancelResize();
    this.resizeObserver.disconnect();
    this.agentState.dispose();
    this.outQueue = [];
    if (this.flushTimer !== null) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    void invoke("pty_kill", { id: this.id, generation: this.generation }).catch(() => {});
    this.term.dispose();
    this.pane.remove();
  }
}
