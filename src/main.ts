import "@xterm/xterm/css/xterm.css";
import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { TabManager } from "./terminal/tabmanager";
import { AgentOverlay } from "./overlay/controller";
import type { TerminalSession } from "./terminal/session";
import { WorkspaceStore } from "./workspaces/store";
import { Sidebar, closeAllMenus } from "./workspaces/sidebar";
import { showToast } from "./ui/toast";
import { showConfirmDialog } from "./ui/dialog";
import { applyLayoutFonts, loadFontSettings, openSettings } from "./ui/settings";
import { normPath, sleep } from "./util";
import type { AgentName, ShellInfo } from "./types";

/** 本次会话内点击"此目录不再提示"的路径集合 */

/** 双通道（输入流检测 + 进程树监控）去重冷却：同 tab 同 agent 60s 内只处理一次 */
const agentCooldown = new Map<string, number>();

function agentFlow(session: TerminalSession | undefined, agent: AgentName, store: WorkspaceStore): void {
  if (!session) return;
  const key = `${session.id}:${agent}`;
  const now = Date.now();
  if (now - (agentCooldown.get(key) ?? 0) < 60_000) return;
  agentCooldown.set(key, now);
  void handleAgent(session, store, agent);
}

/* ---------- 标签栏高度档位 ---------- */

/* ---------- 主流程 ---------- */

async function main(): Promise<void> {
  // 工作区仓库
  const store = new WorkspaceStore();
  await store.init();

  // 全局右键兜底：除输入控件（保留原生剪切/复制/粘贴）外一律屏蔽 WebView2 默认菜单；
  // 已定制区域（终端/标签/工作区卡片）的菜单自行处理并 stopPropagation
  document.addEventListener("contextmenu", (e) => {
    const t = e.target as HTMLElement | null;
    if (t?.closest("input, textarea, [contenteditable]")) return;
    e.preventDefault();
  });

  // Pi 扩展事件信箱：按标签转发到状态机。
  await listen<{ tabId: number; generation: number; kind: string }>("agent-event", (e) => {
    tabs.deliverAgentEvent(e.payload.tabId, e.payload.generation, e.payload.kind);
  });


  // 检测可用 shell，取第一个（pwsh 优先，由 Rust 侧排序保证）
  const shells = await invoke<ShellInfo[]>("list_shells");
  const shell: ShellInfo =
    shells[0] ?? { id: "powershell", name: "PowerShell", program: "powershell.exe", args: [] };
  document.getElementById("status-shell")!.textContent = shell.name;

  const toggleSidebar = () => document.getElementById("sidebar")?.classList.toggle("collapsed");
  document.getElementById("btn-toggle-sidebar")?.addEventListener("click", toggleSidebar);

  const overlay = new AgentOverlay((id) => tabs.activate(id));

  // 多标签终端
  const tabs = new TabManager({
    tabsEl: document.getElementById("tabs")!,
    panesEl: document.getElementById("panes")!,
    shell,
    onAgent: (session, agent) => agentFlow(session, agent, store),
    onSidebarToggle: toggleSidebar,
    onSearchOpen: () => openSearch(),
    onSessionsChange: (manager) => overlay.sync(manager.sessions, manager.active?.id),
    getTermFontSize: () => currentTermFont,
    onSaveToWorkspace: (session) => void saveSessionToWorkspace(session, store, tabs),
    onTabContext: (id, ev) => buildTabContextMenu(tabs, store, id, ev),
    onActiveChange: (session) => {
      sidebar.refresh();
      void session.refreshCwd();
    },
    onCwd: (s) => {
      if (s === tabs.active) sidebar.refresh();
    },
  });

  void overlay.init().then(() => overlay.sync(tabs.sessions, tabs.active?.id)).catch((error) => {
    console.error("悬浮窗初始化失败", error);
    showToast("悬浮窗初始化失败：" + String(error));
  });

  // 侧栏（双击工作区：新标签以工作区名命名并进入该目录，之后不变）
  const sidebar = new Sidebar(store, () => tabs.active, async (ws) => {
    await tabs.newTab(ws.name, ws.path);
  });

  /* ---------- 侧栏宽度拖拽 ---------- */
  const SW_KEY = "lite-sidebar-w";
  const splitterEl = document.getElementById("splitter")!;
  const setSidebarW = (px: number): void => {
    const w = Math.max(180, Math.min(420, Math.round(px)));
    document.documentElement.style.setProperty("--sidebar-w", `${w}px`);
    localStorage.setItem(SW_KEY, String(w));
  };
  const savedW = Number(localStorage.getItem(SW_KEY));
  if (savedW >= 180 && savedW <= 420) setSidebarW(savedW);
  let resizing = false;
  splitterEl.addEventListener("mousedown", (e) => {
    e.preventDefault();
    resizing = true;
    splitterEl.classList.add("dragging");
    document.body.classList.add("resizing");
  });
  document.addEventListener("mousemove", (e) => {
    if (resizing) setSidebarW(e.clientX);
  });
  document.addEventListener("mouseup", () => {
    if (!resizing) return;
    resizing = false;
    splitterEl.classList.remove("dragging");
    document.body.classList.remove("resizing");
    tabs.active?.refit();
  });
  splitterEl.addEventListener("dblclick", () => {
    setSidebarW(250);
  });

  /* ---------- 活动会话目录：OSC 7 优先，其余在可见时查询 PEB ---------- */
  const refreshActiveCwd = (): void => {
    if (document.visibilityState === "visible") void tabs.active?.refreshCwd();
  };
  window.setInterval(refreshActiveCwd, 3000);
  document.addEventListener("visibilitychange", refreshActiveCwd);
  /* ---------- 空状态：新建会话按钮 ---------- */
  document.getElementById("btn-empty-new")?.addEventListener("click", () => void tabs.newTab());

  // 新建标签按钮
  document.getElementById("btn-new-tab")?.addEventListener("click", () => void tabs.newTab());

  // 标签列表（会话多时的「更多」入口）
  document.getElementById("btn-tab-list")?.addEventListener("click", (e) => {
    openTabListMenu(tabs, e.currentTarget as HTMLElement);
  });

  bindSearchUi(tabs);

  /* ---------- 自定义标题栏：窗口控制 ---------- */
  const appWindow = getCurrentWindow();
  document.getElementById("btn-win-min")!.onclick = () => void appWindow.minimize();
  // 关闭软件：有会话时二次确认；无会话直接关
  const closeWithConfirm = (): void => {
    const n = tabs.sessions.length;
    if (n === 0) {
      void appWindow.close();
      return;
    }
    void showConfirmDialog({
      title: "关闭 sTerm",
      message: `有 ${n} 个会话正在运行。\n关闭后所有终端会话将结束。`,
      confirmText: "关闭",
      danger: true,
    }).then((ok) => {
      if (ok) void appWindow.destroy();
    });
  };
  document.getElementById("btn-win-close")!.onclick = closeWithConfirm;
  appWindow.onCloseRequested((e) => {
    if (tabs.sessions.length === 0) return;
    e.preventDefault();
    closeWithConfirm();
  });
  document.getElementById("btn-win-max")!.onclick = () => void appWindow.toggleMaximize();
  // 终端字号：全部会话立即生效并重新适配；新标签创建时读取 currentTermFont
  let currentTermFont = loadFontSettings().term;
  const setTermFont = (px: number): void => {
    currentTermFont = px;
    for (const s of tabs.sessions) {
      s.term.options.fontSize = px;
      s.refit();
    }
    tabs.active?.refit();
  };
  document.getElementById("btn-settings")!.onclick = () =>
    openSettings({ onTermFont: setTermFont, shellId: shell.id });
  applyLayoutFonts(loadFontSettings());
  setTermFont(currentTermFont);

  // 最大化状态 → 切换还原/最大化图标
  const maxBtn = document.getElementById("btn-win-max")!;
  const ICON_MAX =
    '<svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" stroke-width="1" fill="none"><rect x="1.5" y="1.5" width="7" height="7" rx="0.5" /></svg>';
  const ICON_RESTORE =
    '<svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" stroke-width="1" fill="none"><rect x="0.5" y="3.5" width="6" height="6" rx="0.5" /><path d="M3 3.5 V0.5 H9.5 V7 H6.5" /></svg>';
  const syncMaxIcon = async () => {
    maxBtn.innerHTML = (await appWindow.isMaximized()) ? ICON_RESTORE : ICON_MAX;
    maxBtn.title = (await appWindow.isMaximized()) ? "还原" : "最大化";
  };
  void syncMaxIcon();
  appWindow.onResized(() => void syncMaxIcon());

  // 启动时不自动打开会话：显示引导空状态，用户自行新建（Ctrl+T / 双击工作区）
  tabs.updateEmptyState();
}

/* ---------- 查找（作用于当前活动标签） ---------- */

function bindSearchUi(tabs: TabManager): void {
  const input = document.getElementById("search-input") as HTMLInputElement;
  const count = document.getElementById("search-count")!;

  const doSearch = (forward: boolean) => {
    const session = tabs.active;
    if (!session) return;
    const q = input.value;
    if (!q) {
      count.textContent = "";
      return;
    }
    const opts = {
      decorations: {
        matchBackground: "#767676",
        matchOverviewRuler: "#767676",
        activeMatchBackground: "#F9F1A5",
        activeMatchColorOverviewRuler: "#F9F1A5",
      },
    };
    if (forward) session.searchAddon.findNext(q, opts);
    else session.searchAddon.findPrevious(q, opts);
  };

  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      closeSearch(tabs);
    } else if (e.key === "Enter") {
      e.preventDefault();
      doSearch(!e.shiftKey);
    }
  });
  input.addEventListener("input", () => doSearch(true));
  document.getElementById("search-next")!.onclick = () => doSearch(true);
  document.getElementById("search-prev")!.onclick = () => doSearch(false);
  document.getElementById("search-close")!.onclick = () => closeSearch(tabs);
}

function openSearch(): void {
  document.getElementById("search-box")?.classList.remove("hidden");
  const input = document.getElementById("search-input") as HTMLInputElement;
  input.value = "";
  document.getElementById("search-count")!.textContent = "";
  input.focus();
}

function closeSearch(tabs: TabManager): void {
  document.getElementById("search-box")?.classList.add("hidden");
  try {
    tabs.active?.searchAddon.clearDecorations();
  } catch {
    /* ignore */
  }
  tabs.active?.term.focus();
}

/* ---------- 拦截处理 ---------- */

/** Agent 启动后同步已有工作区徽标；目录优先使用 OSC 7，缺少上报时查询 PEB。 */
async function handleAgent(
  session: TerminalSession,
  store: WorkspaceStore,
  agent: AgentName,
): Promise<void> {
  await sleep(400);
  await session.refreshCwd();
  const cwd = session.cwd;
  if (!cwd) return;

  // 已有该工作区：静默合并新 agent 徽标 + 刷新 last_used
  if (store.hasPath(cwd)) {
    if (!store.hasAgent(cwd, agent)) await store.add(cwd, agent);
    return;
  }
  // 新目录不再弹窗打扰：需要保存时经终端/标签右键"保存到工作区"
}

/* ---------- 右键动作：保存到工作区 / 截图 ---------- */

async function saveSessionToWorkspace(
  session: TerminalSession | undefined,
  store: WorkspaceStore,
  tabs: TabManager,
): Promise<void> {
  if (!session) return;
  await session.refreshCwd();
  const cwd = session.cwd;
  if (!cwd) {
    showToast("获取当前目录失败");
    return;
  }
  const existing = store.data.workspaces.find((ws) => normPath(ws.path) === normPath(cwd));
  const workspace = existing ?? await store.add(cwd, session.agentState.agent ?? undefined);
  // 明确保存时与工作区名称同步，日常切换目录不改名。
  session.title = workspace.name;
  tabs.refreshTabs();
  showToast(`${existing ? "已在工作区列表中" : "已保存到工作区"}：\n${cwd}`, [], 3000);
}

/** 标签右键菜单：保存到工作区 / 重命名 / 截图 / 关闭 */
function buildTabContextMenu(
  tabs: TabManager,
  store: WorkspaceStore,
  id: number,
  e: MouseEvent,
): void {
  document.querySelectorAll(".context-menu").forEach((m) => m.remove());
  const session = tabs.byId(id);
  if (!session) return;

  const menu = document.createElement("div");
  menu.className = "context-menu";
  const svg = (paths: string): string =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
  const ICONS = {
    saveWs: svg(
      '<path d="M12 10v6"/><path d="m9 13 3 3 3-3"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 0-1.69.9L9.6 9.9H4a2 2 0 0 0-2 2V18a2 2 0 0 0 2 2Z"/>'
    ),
    rename: svg('<path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/>'),
    close: svg('<path d="M18 6 6 18"/><path d="m6 6 12 12"/>'),
  };
  const mk = (label: string, icon: string, fn: () => void): void => {
    const item = document.createElement("div");
    item.className = "menu-item";
    const iconEl = document.createElement("span");
    iconEl.className = "mi-icon";
    iconEl.innerHTML = icon;
    const labelEl = document.createElement("span");
    labelEl.textContent = label;
    item.appendChild(iconEl);
    item.appendChild(labelEl);
    item.onclick = () => {
      fn();
      menu.remove();
    };
    menu.appendChild(item);
  };

  mk("保存到工作区", ICONS.saveWs, () => void saveSessionToWorkspace(session, store, tabs));
  mk("重命名", ICONS.rename, () => tabs.startTabRename(session));
  const sep = document.createElement("div");
  sep.className = "menu-sep";
  menu.appendChild(sep);
  mk("关闭标签", ICONS.close, () => tabs.closeTab(id));

  document.body.appendChild(menu);
  const w = menu.offsetWidth || 160;
  const h = menu.offsetHeight || 140;
  menu.style.left = `${Math.max(8, Math.min(e.clientX, window.innerWidth - w - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(e.clientY, window.innerHeight - h - 8))}px`;
}

main().catch((e) => {
  const el = document.getElementById("ws-empty");
  if (el) el.innerHTML = `启动失败：${String(e)}`;
  console.error(e);
});

/* ---------- 标签列表菜单（会话多时的「更多」入口） ---------- */

function openTabListMenu(tabs: TabManager, anchor: HTMLElement): void {
  closeAllMenus();
  if (tabs.sessions.length === 0) return;

  const menu = document.createElement("div");
  menu.className = "context-menu tab-list-menu";

  const head = document.createElement("div");
  head.className = "import-header";
  head.textContent = `打开的标签（${tabs.sessions.length}）`;
  menu.appendChild(head);

  for (const s of tabs.sessions) {
    const row = document.createElement("div");
    row.className = "menu-item tab-list-item" + (s.activeFlag ? " current" : "");

    const num = document.createElement("span");
    num.className = "tab-list-num";
    num.textContent = String(tabs.sessions.indexOf(s) + 1);
    row.appendChild(num);

    const label = document.createElement("span");
    label.className = "tab-list-label";
    label.textContent = s.title;
    label.title = s.title;
    row.appendChild(label);

    if (s.activeFlag) {
      const cur = document.createElement("span");
      cur.className = "tab-list-cur";
      cur.textContent = "当前";
      row.appendChild(cur);
    }

    const close = document.createElement("button");
    close.className = "tab-list-close";
    close.textContent = "✕";
    close.title = "关闭标签";
    close.onclick = (e) => {
      e.stopPropagation();
      tabs.closeTab(s.id);
      closeAllMenus();
    };
    row.appendChild(close);

    row.onclick = () => {
      tabs.activate(s.id);
      closeAllMenus();
    };
    menu.appendChild(row);
  }

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  const width = menu.offsetWidth || 220;
  menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - width - 8))}px`;
  menu.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - 120)}px`;
}
