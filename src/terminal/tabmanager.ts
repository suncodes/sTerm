import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { b64ToU8 } from "../util";
import { TerminalSession } from "./session";
import type { AgentName, ShellInfo } from "../types";

interface OutputPayload {
  id: number;
  generation: number;
  data: string;
}

interface ExitPayload {
  id: number;
  generation: number;
}

export interface TabManagerOptions {
  /** 状态、名称、活动标签及会话增删的统一通知。 */
  onSessionsChange?: (tabs: TabManager) => void;
  tabsEl: HTMLElement;
  panesEl: HTMLElement;
  shell: ShellInfo;
  onAgent: (session: TerminalSession, agent: AgentName) => void;
  onSidebarToggle: () => void;
  onSearchOpen: () => void;

  /** 新标签的终端字号（来自设置） */
  getTermFontSize?: () => number;
  /** shell integration cwd 变化 */
  onCwd?: (session: TerminalSession, cwd: string) => void;
  /** 标签右键（id + 鼠标事件，用于构建标签专属菜单） */
  onTabContext?: (id: number, e: MouseEvent) => void;
  /** 活跃标签切换（用于侧栏工作区高亮刷新等） */
  onActiveChange?: (session: TerminalSession) => void;
  /** 右键"保存到工作区"（透传给 TerminalSession） */
  onSaveToWorkspace?: (session: TerminalSession) => void;
}

/** 多标签管理：标签条 UI + 会话生命周期 + PTY 事件分发 */
export class TabManager {
  readonly sessions: TerminalSession[] = [];
  private activeIdx = 0;
  private nextId = 1;
  private opts: TabManagerOptions;
  private listenersReady: Promise<UnlistenFn[]>;

  constructor(opts: TabManagerOptions) {
    this.opts = opts;

    // 全局 Ctrl+T：空状态（无会话）时也能新建（会话内的由 session 自己处理）
    document.addEventListener("keydown", (e) => {
      if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "t") {
        // 会话存在时由 session 的 handler 处理（那里已 preventDefault），这里免重复
        if (this.sessions.length === 0) {
          e.preventDefault();
          void this.newTab();
        }
      }
    });

    // 全局 PTY 事件 → 按会话 id 分发
    this.listenersReady = Promise.all([
      listen<OutputPayload>("pty-output", (e) => {
        this.byId(e.payload.id)?.handleOutput(e.payload.generation, b64ToU8(e.payload.data));
      }),
      listen<ExitPayload>("pty-exit", (e) => {
        this.byId(e.payload.id)?.handleExit(e.payload.generation);
      }),
    ]);
  }

  byId(id: number): TerminalSession | undefined {
    return this.sessions.find((s) => s.id === id);
  }

  get active(): TerminalSession {
    return this.sessions[this.activeIdx];
  }

  /** 未指定目录时使用默认目录；标题仅在保存工作区或手工重命名时更新。 */
  async newTab(title?: string, cwd?: string): Promise<TerminalSession> {
    const session = new TerminalSession({
      id: this.nextId++,
      panesEl: this.opts.panesEl,
      shell: this.opts.shell,
      cwd: cwd ?? null,
      fontSize: this.opts.getTermFontSize?.(),
      title,
      onCwd: this.opts.onCwd,
      onAgent: this.opts.onAgent,
      onSaveToWorkspace: this.opts.onSaveToWorkspace,
      onAgentStatus: (session) => this.handleAgentStatus(session),
      onSidebarToggle: this.opts.onSidebarToggle,
      onNewTab: () => void this.newTab(),
      onCloseTab: () => this.closeActive(),
      onSearchOpen: this.opts.onSearchOpen,
    });
    this.sessions.push(session);

    // 搜索结果计数（仅活动标签更新显示）
    const addon = session.searchAddon as unknown as {
      onDidChangeResults?: (cb: (ev: { resultIndex: number; resultCount: number } | undefined) => void) => void;
    };
    addon.onDidChangeResults?.((ev) => {
      const el = document.getElementById("search-count");
      if (el && session.isActive) {
        el.textContent = ev && ev.resultCount > 0 ? `${ev.resultIndex + 1}/${ev.resultCount}` : "";
      }
    });

    // 显示面板后测量真实尺寸，监听就绪后才启动，避免漏掉早期输出/退出事件。
    this.activate(session.id);
    this.updateEmptyState();
    await this.listenersReady;
    await session.start();
    await session.refreshCwd();
    return session;
  }

  closeActive(): void {
    if (this.active) this.closeTab(this.active.id);
  }

  closeTab(id: number): void {
    const idx = this.sessions.findIndex((s) => s.id === id);
    if (idx < 0) return;
    this.sessions[idx].dispose();
    this.sessions.splice(idx, 1);

    // 允许关到 0 个：显示空状态
    if (this.sessions.length === 0) {
      this.activeIdx = 0;
      this.renderTabs();
      this.updateEmptyState();
      const tabsEl = document.getElementById("status-tabs");
      if (tabsEl) tabsEl.textContent = "0 个标签";
      return;
    }
    if (this.activeIdx >= this.sessions.length) {
      this.activeIdx = this.sessions.length - 1;
    } else if (idx < this.activeIdx) {
      this.activeIdx--;
    }
    this.activate(this.active.id);
  }

  /** 空状态显隐（无任何会话时主区域显示引导页） */
  /** 供启动流程调用：无会话时显示引导空状态 */
  updateEmptyState(): void {
    document.getElementById("empty-state")?.classList.toggle("hidden", this.sessions.length > 0);
  }

  /** 事件信箱：按标签 id 转发 hook 事件（prompt/stop/end） */
  deliverAgentEvent(id: number, generation: number, kind: string): void {
    this.byId(id)?.deliverAgentEvent(generation, kind);
  }

  /** Agent 状态变化：重绘标签状态点 + 同步状态栏 */
  handleAgentStatus(session: TerminalSession): void {
    this.renderTabs();
    if (session === this.active) this.updateStatusAgent(session);
  }

  /** 状态栏：当前活跃会话的 agent 状态 */
  private updateStatusAgent(session: TerminalSession): void {
    const el = document.getElementById("status-agent");
    if (!el) return;
    const st = session.agentState.status;
    const name = session.agentState.agent;
    const label = { running: "运行中", idle: "空闲", exited: "已退出", none: "" }[st] ?? "";
    el.textContent = st !== "none" && name ? `${name} ${label}` : "";
    el.dataset.status = st;
  }

  /** 会话标题变更后重绘标签条 */
  refreshTabs(): void {
    this.renderTabs();
  }

  activate(id: number): void {
    const idx = this.sessions.findIndex((s) => s.id === id);
    if (idx < 0) return;
    this.activeIdx = idx;
    for (const s of this.sessions) s.setActive(s.id === id);
    const cur = this.sessions.find((x) => x.id === id);
    if (cur) this.opts.onActiveChange?.(cur);
    this.renderTabs();
    this.active?.refit();
    const tabsEl = document.getElementById("status-tabs");
    if (tabsEl) tabsEl.textContent = `${this.sessions.length} 个标签`;
    if (this.active) this.updateStatusAgent(this.active);
  }

  /* ---------- 标签条渲染 ---------- */

  private renderTabs(): void {
    this.opts.onSessionsChange?.(this);
    const el = this.opts.tabsEl;
    el.innerHTML = "";
    for (const s of this.sessions) {
      const tab = document.createElement("div");
      tab.className = "tab" + (s.isActive ? " active" : "");
      tab.title = s.title;
      tab.dataset.sessionId = String(s.id);
      tab.onclick = () => this.activate(s.id);
      tab.onauxclick = (e) => {
        if (e.button === 1) this.closeTab(s.id); // 中键关闭
      };

      // Agent 状态点（none 时不渲染）
      if (s.agentState.status !== "none") {
        const dot = document.createElement("span");
        dot.className = "tab-state st-" + s.agentState.status;
        dot.title = { running: "运行中", idle: "任务完成", exited: "已退出", none: "" }[s.agentState.status] ?? "";
        tab.appendChild(dot);
      }

      tab.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        this.opts.onTabContext?.(s.id, ev);
      });

      const title = document.createElement("span");
      title.className = "tab-label";
      title.textContent = s.title;
      title.ondblclick = (e) => {
        e.stopPropagation();
        this.startTabRename(s);
      };
      tab.appendChild(title);

      const close = document.createElement("span");
      close.className = "tab-close";
      close.textContent = "✕";
      close.title = "关闭 (Ctrl+W)";
      close.onclick = (e) => {
        e.stopPropagation();
        this.closeTab(s.id);
      };
      tab.appendChild(close);

      el.appendChild(tab);
    }
  }

  /** 标签重命名（双击标签名）：快照语义，改完固定不变 */
  startTabRename(s: TerminalSession): void {
    const label = this.opts.tabsEl.querySelector<HTMLElement>(
      '.tab[data-session-id="' + s.id + '"] .tab-label',
    );
    if (!label) return;
    const input = document.createElement("input");
    input.className = "tab-rename";
    input.value = s.title;
    input.spellcheck = false;
    label.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (commit: boolean): void => {
      if (done) return;
      done = true;
      const v = input.value.trim();
      if (commit && v && v !== s.title) s.title = v;
      this.renderTabs();
    };
    input.onblur = () => finish(true);
    input.onkeydown = (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        finish(true);
      } else if (e.key === "Escape") {
        e.preventDefault();
        finish(false);
      }
    };
  }

  dispose(): void {
    void this.listenersReady.then((listeners) => listeners.forEach((unlisten) => unlisten()));
    for (const s of this.sessions) s.dispose();
    this.sessions.length = 0;
  }
}
