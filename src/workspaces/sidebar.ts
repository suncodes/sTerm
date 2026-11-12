import { invoke } from "@tauri-apps/api/core";
import { isPathWithin, relTime } from "../util";

/** 路径截短：首段(盘符) + … + 尾两级目录（完整路径在 tooltip） */
function shortPath(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  if (parts.length <= 4) return p;
  return parts[0] + "\\…" + parts.slice(-2).join("\\");
}
import { showToast } from "../ui/toast";
import { showInputDialog, showConfirmDialog } from "../ui/dialog";
import type { AgentSession, Folder, ImportCandidate, Workspace } from "../types";
import type { WorkspaceStore } from "./store";
import type { TerminalSession } from "../terminal/session";

const sortWs = (a: Workspace, b: Workspace) =>
  Number(b.pinned) - Number(a.pinned) || b.last_used - a.last_used;

/** 工作区侧栏：分组渲染、拖拽/菜单移动、文件夹 CRUD、导入、会话展开 */
export class Sidebar {
  /** 会话列表缓存（展开状态，随数据刷新重建 DOM 后需重新展开） */
  private expanded = new Map<string, AgentSession[] | "loading" | "empty">();
  /** 工作区是否有历史会话（缓存，决定是否渲染展开箭头） */
  private sessionHas = new Map<string, boolean>();
  private sessionFlagsAt = 0;

  constructor(
    private store: WorkspaceStore,
    private getActiveSession: () => TerminalSession | undefined,
    /** 双击工作区时：新建会话并 cd（由 main 注入 TabManager 能力） */
    private openInNewTab?: (ws: Workspace) => Promise<void>,
  ) {
    store.onChange(() => this.render());
    document.getElementById("btn-add-folder")?.addEventListener("click", () => {
      this.promptNewFolder(null);
    });
    document
      .getElementById("btn-import-ws")
      ?.addEventListener("click", (e) => this.openImportMenu(e.clientX, e.clientY));
    this.render();
  }

  /** 新建文件夹（parentId 为 null 时创建根级） */
  private promptNewFolder(parentId: string | null): void {
    const isSub = parentId !== null;
    const parentName = isSub
      ? this.store.data.folders.find((f) => f.id === parentId)?.name
      : null;
    showInputDialog({
      title: isSub ? "新建子文件夹" : "新建文件夹",
      description: isSub ? `将在「${parentName ?? ""}」下创建` : "用于对工作区分组管理",
      placeholder: "输入文件夹名称",
      confirmText: "创建",
      onConfirm: async (name) => {
        await this.store.folderAdd(name, parentId);
      },
    });
  }

  /** 外部触发重绘（如活跃会话 cwd 变化后的高亮刷新） */
  refresh(): void {
    this.render();
  }

  /** 查询各工作区是否拥有历史会话；与缓存不同则重渲染（有会话才显示 ▸） */
  private async refreshSessionFlags(): Promise<void> {
    const now = Date.now();
    if (now - this.sessionFlagsAt < 5000) return;
    this.sessionFlagsAt = now;
    const list = this.store.data.workspaces;
    if (!list.length) return;
    try {
      const flags = await invoke<boolean[]>("session_counts", {
        paths: list.map((w) => w.path),
      });
      let changed = false;
      list.forEach((w, i) => {
        const has = !!flags[i];
        if (this.sessionHas.get(w.id) !== has) {
          this.sessionHas.set(w.id, has);
          changed = true;
        }
      });
      if (changed) this.render();
    } catch {
      /* 查询失败保持现状 */
    }
  }

  private render(): void {
    const list = document.getElementById("ws-list")!;
    const { folders, workspaces } = this.store.data;

    const rootItems = workspaces.filter((w) => !w.folder_id).sort(sortWs);
    document.getElementById("ws-empty")!.style.display =
      workspaces.length === 0 ? "" : "none";

    list.innerHTML = "";

    // 根级：未分组头（只要存在文件夹就显示，作为拖回根级的投放目标）
    if (folders.length > 0) {
      list.appendChild(this.renderGroupHeader(null, rootItems.length, 0));
    }
    for (const ws of rootItems) list.appendChild(this.renderItem(ws));

    // 根级文件夹（递归渲染子树）
    for (const folder of folders.filter((f) => !f.parent_id)) {
      list.appendChild(this.renderFolder(folder, 0));
    }

    // 根级空白处右键：新建文件夹 / 扫描导入
    list.oncontextmenu = (e) => {
      const t = e.target as HTMLElement;
      if (t.closest(".ws-item") || t.closest(".folder-header")) return;
      e.preventDefault();
      closeAllMenus();
      const menu = this.buildContextMenu([
        { label: "新建根文件夹", icon: Sidebar.ICONS.folderPlus, fn: () => this.promptNewFolder(null) },
        { label: "扫描历史工作区…", icon: Sidebar.ICONS.search, fn: () => void this.openImportMenu(e.clientX, e.clientY) },
      ]);
      this.showMenuAt(menu, e.clientX, e.clientY);
    };

    closeAllMenus();
    // 异步刷新"是否有历史会话"标志（节流 5s；结果变化才重渲染）
    void this.refreshSessionFlags();
  }

  /* ---------- 工作区条目 ---------- */

  private renderItem(ws: Workspace): HTMLElement {
    const row = document.createElement("div");
    row.className = "ws-item";
    // 当前活跃会话所在的工作区：左侧强调线
    const cur = this.getActiveSession()?.cwd;
    if (cur && isPathWithin(cur, ws.path)) {
      row.classList.add("active");
    }
    row.draggable = true;

    row.ondragstart = (e) => {
      if (e.dataTransfer) {
        e.dataTransfer.setData("text/lite-ws", ws.id);
        e.dataTransfer.effectAllowed = "move";
      }
    };

    // 左侧固定圆点列：按名称排列关联的 Agent，未关联时显示灰点。
    const agents = [...ws.agents].sort();
    const dots = document.createElement("span");
    dots.className = "ws-dots";
    dots.title = agents.length ? agents.join(" / ") : "暂无关联 Agent";
    dots.setAttribute("role", "img");
    dots.setAttribute("aria-label", dots.title);
    for (const agent of agents.length ? agents : ["none"]) {
      const dot = document.createElement("span");
      dot.className = `agent-dot agent-${agent}`;
      dot.title = agent === "none" ? dots.title : agent;
      dots.appendChild(dot);
    }
    row.appendChild(dots);

    const top = document.createElement("div");
    top.className = "ws-top";
    const name = document.createElement("span");
    name.className = "ws-name";
    name.textContent = ws.name;
    name.title = ws.name;
    top.appendChild(name);

    if (ws.pinned) {
      const pin = document.createElement("span");
      pin.className = "ws-pin";
      pin.title = "已置顶";
      pin.innerHTML =
        '<svg viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M12 2l2.9 6.26 6.6.57-5 4.36 1.5 6.45L12 16.9l-6 2.74 1.5-6.45-5-4.36 6.6-.57z"/></svg>';
      top.appendChild(pin);
    }

    // hover 仅保留主操作（新会话 + 号，与标签栏新建同语义）；其余操作收进右键菜单
    const ops = document.createElement("span");
    ops.className = "ws-ops";
    const newBtn = document.createElement("button");
    newBtn.className = "ws-new-tab";
    newBtn.title = "新建会话并进入（同双击）";
    newBtn.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>';
    newBtn.onclick = (e) => {
      e.stopPropagation();
      if (this.openInNewTab) void this.openInNewTab(ws);
      else this.getActiveSession()?.changeDirectory(ws.path);
    };
    ops.appendChild(newBtn);
    top.appendChild(ops);
    row.appendChild(top);

    // 行 2：路径（左，淡）+ 时间（右，更淡）+ 展开箭头（仅有历史会话时渲染）
    const sub = document.createElement("div");
    sub.className = "ws-sub";
    const path = document.createElement("span");
    path.className = "ws-path";
    path.textContent = shortPath(ws.path);
    path.title = ws.path;
    sub.appendChild(path);

    const time = document.createElement("span");
    time.className = "ws-time";
    time.textContent = relTime(ws.last_used);
    sub.appendChild(time);

    if (this.sessionHas.get(ws.id)) {
      const expanded = this.expanded.has(ws.id);
      const caret = document.createElement("button");
      caret.className = "session-caret" + (expanded ? " on" : "");
      caret.innerHTML = expanded
        ? '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>'
        : '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>';
      caret.title = "展开/收起历史会话";
      caret.onclick = (e) => {
        e.stopPropagation();
        void this.toggleSessions(ws);
      };
      sub.appendChild(caret);
    }
    row.appendChild(sub);

    // 会话子列表容器
    const sessEl = document.createElement("div");
    sessEl.className = "ws-sessions";
    const cached = this.expanded.get(ws.id);
    if (cached !== undefined) {
      this.fillSessions(sessEl, ws, cached);
    } else {
      sessEl.classList.add("hidden");
    }
    row.appendChild(sessEl);

    // 双击卡片 = 新建会话并进入该目录（单击无操作，展开走 ▸/▾ 按钮，无延迟）
    row.title = "双击：新建会话进入 · ▸：历史会话";
    row.ondblclick = () => {
      if (this.openInNewTab) {
        void this.openInNewTab(ws);
      } else {
        this.getActiveSession()?.changeDirectory(ws.path);
      }
    };
    row.oncontextmenu = (e) => this.showWsContextMenu(e, ws, row);
    return row;
  }

  /* ---------- 会话展开（M3） ---------- */

  private async toggleSessions(ws: Workspace): Promise<void> {
    if (this.expanded.has(ws.id)) {
      this.expanded.delete(ws.id);
      this.render();
      return;
    }
    this.expanded.set(ws.id, "loading");
    this.render();
    const sessions = await this.store.listSessions(ws.path, 5).catch(() => null);
    // 用户可能在加载期间又点了收起
    if (!this.expanded.has(ws.id) || this.expanded.get(ws.id) !== "loading") return;
    this.expanded.set(ws.id, sessions && sessions.length > 0 ? sessions : "empty");
    this.render();
    // 卡片徽标与实际会话保持一致（只合并，不动 last_used）
    if (sessions && sessions.length > 0) {
      const found = Array.from(new Set(sessions.map((x) => x.agent)));
      void this.store.syncAgents(ws.path, found).catch(() => {});
    }
  }

  private fillSessions(el: HTMLElement, ws: Workspace, data: AgentSession[] | "loading" | "empty"): void {
    el.innerHTML = "";
    if (data === "loading") {
      const d = document.createElement("div");
      d.className = "session-empty";
      d.textContent = "加载会话中…";
      el.appendChild(d);
      return;
    }
    if (data === "empty") {
      const d = document.createElement("div");
      d.className = "session-empty";
      d.textContent = "无历史会话";
      el.appendChild(d);
      return;
    }
    for (const s of data) {
      const item = document.createElement("div");
      item.className = "session-item";
      const label = s.title?.trim() || `#${s.session_id.slice(0, 8)}`;
      item.title = `${s.agent} 会话：${label}\n${s.session_id}\n点击在终端中 resume`;

      const dot = document.createElement("span");
      dot.className = `agent-dot agent-${s.agent}`;
      item.appendChild(dot);

      const name = document.createElement("span");
      name.className = "session-title";
      name.textContent = label;
      item.appendChild(name);

      const t = document.createElement("span");
      t.className = "session-time";
      t.textContent = relTime(s.last_used);
      item.appendChild(t);

      const hint = document.createElement("span");
      hint.className = "session-hint";
      hint.textContent = "resume ↵";
      item.appendChild(hint);

      item.onclick = (e) => {
        e.stopPropagation();
        const s2 = this.getActiveSession();
        if (!s2) return;
        const cmd =
          s.agent === "claude"
            ? `claude --resume ${s.session_id}`
            : s.agent === "pi"
              ? `pi --session ${s.session_id}`
              : `codex resume ${s.session_id}`;
        void (async () => {
          await s2.changeDirectory(ws.path);
          await s2.writeCommand(cmd);
        })();
      };
      el.appendChild(item);
    }
  }

  /* ---------- 文件夹（多级） ---------- */

  private renderFolder(folder: Folder, depth: number): HTMLElement {
    const box = document.createElement("div");
    box.className = "folder" + (folder.collapsed ? " collapsed" : "");

    const { folders, workspaces } = this.store.data;
    const items = workspaces
      .filter((w) => w.folder_id === folder.id)
      .sort(sortWs);

    const header = this.renderGroupHeader(folder, items.length, depth);
    box.appendChild(header);

    const body = document.createElement("div");
    body.className = "folder-items";
    for (const ws of items) body.appendChild(this.renderItem(ws));
    for (const child of folders.filter((f) => f.parent_id === folder.id)) {
      body.appendChild(this.renderFolder(child, depth + 1));
    }
    box.appendChild(body);
    return box;
  }

  /**
   * 分组头：文件夹或"未分组"（folder 为 null）。
   * 支持三种拖放落点：上 30% 插到前面，下 30% 插到后面，中 40% 放入内部。
   */
  private renderGroupHeader(folder: Folder | null, count: number, depth: number): HTMLElement {
    const header = document.createElement("div");
    // 未分组：轻分隔线（非文件夹容器语义）；文件夹：常规组头
    header.className = (folder ? "folder-header" : "root-sep") + " drop-zone";
    header.style.paddingLeft = folder ? `${9 + depth * 14}px` : "";

    if (folder) {
      const caret = document.createElement("span");
      caret.className = "folder-caret";
      caret.textContent = "▾";
      header.appendChild(caret);

      const nameEl = document.createElement("span");
      nameEl.className = "folder-name";
      nameEl.textContent = folder.name;
      nameEl.title = folder.name;
      header.appendChild(nameEl);

      const cnt = document.createElement("span");
      cnt.className = "folder-count";
      cnt.textContent = String(count);
      header.appendChild(cnt);
    } else {
      const line = document.createElement("span");
      line.className = "root-sep-line";
      header.appendChild(line);
      const label = document.createElement("span");
      label.className = "root-sep-label";
      label.textContent = "未分组";
      header.appendChild(label);
    }

    if (folder) {
      header.draggable = true;
      header.ondragstart = (e) => {
        if (e.dataTransfer) {
          e.dataTransfer.setData("text/lite-folder", folder.id);
          e.dataTransfer.effectAllowed = "move";
        }
      };

      const ops = document.createElement("span");
      ops.className = "ws-ops";
      const mk = (icon: string, title: string, fn: () => void, extra?: string) => {
        const b = document.createElement("button");
        b.textContent = icon;
        b.title = title;
        if (extra) b.classList.add(extra);
        b.onclick = (e) => {
          e.stopPropagation();
          fn();
        };
        ops.appendChild(b);
      };
      mk("＋", "新建子文件夹", () => this.promptNewFolder(folder.id));
      mk("✎", "重命名文件夹", () => this.startFolderRename(header, folder));
      mk("✕", "删除文件夹（内容提升到上一级）", () => void this.confirmRemoveFolder(folder), "del");
      header.appendChild(ops);

      header.onclick = () => void this.store.folderToggle(folder.id, !folder.collapsed);
      header.ondblclick = () => this.startFolderRename(header, folder);
      header.oncontextmenu = (e) => this.showFolderContextMenu(e, folder, header);
    }

    // 拖放落点计算
    const dropPos = (e: DragEvent): "before" | "after" | "into" => {
      const rect = header.getBoundingClientRect();
      const rel = (e.clientY - rect.top) / rect.height;
      return rel < 0.3 ? "before" : rel > 0.7 ? "after" : "into";
    };
    header.ondragover = (e) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
      const pos = dropPos(e);
      header.classList.toggle("drop-into", pos === "into");
      header.classList.toggle("drop-line-top", pos === "before");
      header.classList.toggle("drop-line-bottom", pos === "after");
    };
    header.ondragleave = () => {
      header.classList.remove("drop-into", "drop-line-top", "drop-line-bottom");
    };
    header.ondrop = (e) => {
      e.preventDefault();
      header.classList.remove("drop-into", "drop-line-top", "drop-line-bottom");
      const wsId = e.dataTransfer?.getData("text/lite-ws");
      const folderId = e.dataTransfer?.getData("text/lite-folder");
      if (wsId) {
        // 工作区：放入文件夹（或未分组 = 根级）
        void this.store.move(wsId, folder ? folder.id : null);
        return;
      }
      if (folderId && folderId !== folder?.id) {
        const pos = dropPos(e);
        if (!folder) {
          // 拖到「未分组」= 移回根级末尾
          void this.store.folderMove(folderId, null, null);
          return;
        }
        if (pos === "into") {
          void this.store.folderMove(folderId, folder.id, null);
          return;
        }
        // before / after：目标文件夹的同父级序列
        const siblings = this.store.data.folders.filter(
          (f) => (f.parent_id ?? null) === (folder.parent_id ?? null),
        );
        if (pos === "before") {
          void this.store.folderMove(folderId, folder.parent_id ?? null, folder.id);
        } else {
          const idx = siblings.findIndex((f) => f.id === folder.id);
          const next = siblings[idx + 1];
          void this.store.folderMove(folderId, folder.parent_id ?? null, next ? next.id : null);
        }
      }
    };

    return header;
  }

  /* ---------- 删除二次确认 ---------- */

  private async confirmRemoveWs(ws: Workspace): Promise<void> {
    const ok = await showConfirmDialog({
      title: "移除工作区",
      message: `确定移除「${ws.name}」吗？\n仅从列表移除，不影响磁盘上的目录。`,
      confirmText: "移除",
      danger: true,
    });
    if (ok) await this.store.remove(ws.id);
  }

  private async confirmRemoveFolder(folder: Folder): Promise<void> {
    const ok = await showConfirmDialog({
      title: "删除文件夹",
      message: `确定删除「${folder.name}」吗？\n其中的子文件夹与工作区将提升到上一级，不会丢失。`,
      confirmText: "删除",
      danger: true,
    });
    if (ok) await this.store.folderRemove(folder.id);
  }

  /* ---------- 右键菜单 ---------- */

/** 菜单图标（lucide stroke 风格，与终端/标签菜单一致） */  private static readonly ICONS = {
  folderPlus:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 0-1.69.9L9.6 9.9H4a2 2 0 0 0-2 2V18a2 2 0 0 0 2 2Z"/><path d="M12 11v4"/><path d="M10 13h4"/></svg>',
  search:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>',
  pin: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>',
  pinOff:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/><line x1="2" x2="22" y1="2" y2="22"/></svg>',
  pencil:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>',
  chevronDown:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>',
  chevronRight:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>',
  moveTo:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>',
  folder:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 0-1.69.9L9.6 9.9H4a2 2 0 0 0-2 2V18a2 2 0 0 0 2 2Z"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>',
};

  private buildContextMenu(
    items: {
      label?: string;
      icon?: string;
      fn?: () => void;
      danger?: boolean;
      sep?: boolean;
    }[],
  ): HTMLElement {
    const menu = document.createElement("div");
    menu.className = "context-menu";
    for (const it of items) {
      if (it.sep) {
        const sep = document.createElement("div");
        sep.className = "menu-sep";
        menu.appendChild(sep);
        continue;
      }
      const item = document.createElement("div");
      item.className = "menu-item" + (it.danger ? " danger" : "");
      if (it.icon) {
        const iconEl = document.createElement("span");
        iconEl.className = "mi-icon";
        iconEl.innerHTML = it.icon;
        item.appendChild(iconEl);
      }
      const labelEl = document.createElement("span");
      labelEl.textContent = it.label!;
      item.appendChild(labelEl);
      item.onclick = () => {
        it.fn?.();
        closeAllMenus();
      };
      menu.appendChild(item);
    }
    return menu;
  }

  /** 在坐标处显示菜单（自动防出屏） */
  private showMenuAt(menu: HTMLElement, x: number, y: number): void {
    document.body.appendChild(menu);
    const w = menu.offsetWidth || 200;
    const h = menu.offsetHeight || 100;
    menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - w - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - h - 8))}px`;
  }

  private showFolderContextMenu(e: MouseEvent, folder: Folder, header: HTMLElement): void {
    e.preventDefault();
    e.stopPropagation();
    closeAllMenus();
    const menu = this.buildContextMenu([
      { label: "新建子文件夹", icon: Sidebar.ICONS.folderPlus, fn: () => this.promptNewFolder(folder.id) },
      { label: "重命名", icon: Sidebar.ICONS.pencil, fn: () => this.startFolderRename(header, folder) },
      {
        label: folder.collapsed ? "展开" : "折叠",
        icon: folder.collapsed ? Sidebar.ICONS.chevronDown : Sidebar.ICONS.chevronRight,
        fn: () => void this.store.folderToggle(folder.id, !folder.collapsed),
      },
      { sep: true },
      {
        label: "删除文件夹（内容提升到上一级）",
        icon: Sidebar.ICONS.x,
        fn: () => void this.confirmRemoveFolder(folder),
        danger: true,
      },
    ]);
    this.showMenuAt(menu, e.clientX, e.clientY);
  }

  private showWsContextMenu(e: MouseEvent, ws: Workspace, row: HTMLElement): void {
    e.preventDefault();
    e.stopPropagation();
    closeAllMenus();
    const menu = this.buildContextMenu([
      {
        label: ws.pinned ? "取消置顶" : "置顶",
        icon: ws.pinned ? Sidebar.ICONS.pinOff : Sidebar.ICONS.pin,
        fn: () => void this.store.pin(ws.id, !ws.pinned),
      },
      { label: "重命名", icon: Sidebar.ICONS.pencil, fn: () => this.startRename(row, ws) },
      { label: "移动到文件夹…", icon: Sidebar.ICONS.moveTo, fn: () => this.openMoveMenu(ws, row) },
      { sep: true },
      { label: "移除工作区", icon: Sidebar.ICONS.x, fn: () => void this.confirmRemoveWs(ws), danger: true },
    ]);
    this.showMenuAt(menu, e.clientX, e.clientY);
  }

  /* ---------- 移动菜单 ---------- */

  private openMoveMenu(ws: Workspace, anchor: HTMLElement): void {
    closeAllMenus();
    const menu = document.createElement("div");
    menu.className = "context-menu";

    const mk = (label: string, icon: string | null, fn: () => void) => {
      const item = document.createElement("div");
      item.className = "menu-item";
      if (icon) {
        const iconEl = document.createElement("span");
        iconEl.className = "mi-icon";
        iconEl.innerHTML = icon;
        item.appendChild(iconEl);
      }
      const labelEl = document.createElement("span");
      labelEl.textContent = label;
      item.appendChild(labelEl);
      item.onclick = () => {
        fn();
        closeAllMenus();
      };
      menu.appendChild(item);
    };

    mk("未分组（根级）", Sidebar.ICONS.folder, () => void this.store.move(ws.id, null));
    const pathOf = (id: string): string => {
      const { folders } = this.store.data;
      const byId = new Map(folders.map((f) => [f.id, f]));
      const parts: string[] = [];
      let cur = byId.get(id);
      while (cur) {
        parts.unshift(cur.name);
        cur = cur.parent_id ? byId.get(cur.parent_id) : undefined;
      }
      return parts.join(" / ");
    };
    for (const f of this.store.data.folders) {
      mk(pathOf(f.id), Sidebar.ICONS.folder, () => void this.store.move(ws.id, f.id));
    }

    const sep = document.createElement("div");
    sep.className = "menu-sep";
    menu.appendChild(sep);

    const newFolder = document.createElement("div");
    newFolder.className = "menu-item";
    {
      const iconEl = document.createElement("span");
      iconEl.className = "mi-icon";
      iconEl.innerHTML = Sidebar.ICONS.folderPlus;
      newFolder.appendChild(iconEl);
      const labelEl = document.createElement("span");
      labelEl.textContent = "新建文件夹并移入";
      newFolder.appendChild(labelEl);
    }
    newFolder.onclick = () => {
      const input = document.createElement("input");
      input.className = "menu-input";
      input.placeholder = "文件夹名称，回车确认";
      newFolder.replaceWith(input);
      input.focus();
      input.onkeydown = (e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          const name = input.value.trim();
          if (name) {
            void (async () => {
              const f = await this.store.folderAdd(name);
              await this.store.move(ws.id, f.id);
            })();
          }
          closeAllMenus();
        } else if (e.key === "Escape") {
          closeAllMenus();
        }
      };
    };
    menu.appendChild(newFolder);

    document.body.appendChild(menu);
    positionMenu(menu, anchor);
  }

  /* ---------- 导入历史工作区（M2） ---------- */

  private async openImportMenu(x: number, y: number): Promise<void> {
    closeAllMenus();
    const menu = document.createElement("div");
    menu.className = "context-menu import-menu";
    const loading = document.createElement("div");
    loading.className = "menu-item dim";
    loading.textContent = "正在扫描 ~/.claude 与 ~/.codex …";
    menu.appendChild(loading);
    this.showMenuAt(menu, x, y);

    const list = await this.store.importScan().catch(() => null);
    closeAllMenus();
    if (list === null) {
      showToast("扫描失败，请稍后重试", [{ label: "关闭", onClick: () => {} }]);
      return;
    }
    if (list.length === 0) {
      showToast("未发现历史工作区（~/.claude/projects 与 ~/.codex/sessions 为空）", [
        { label: "关闭", onClick: () => {} },
      ]);
      return;
    }

    const m = document.createElement("div");
    m.className = "context-menu import-menu";
    const imported = new Set<string>();

    // 头部：标题 + 全部导入
    const head = document.createElement("div");
    head.className = "import-header";
    const title = document.createElement("span");
    title.className = "dim";
    title.textContent = `发现 ${list.length} 个历史工作区`;
    const allBtn = document.createElement("button");
    allBtn.className = "menu-ok";
    allBtn.textContent = "全部导入";
    allBtn.onclick = async () => {
      const rest = list.filter((c) => !imported.has(c.path.toLowerCase()));
      const n = await this.store.importWs(rest).catch(() => 0);
      closeAllMenus();
      showToast(`已导入 ${n} 个工作区`, [{ label: "好的", onClick: () => {} }]);
    };
    head.append(title, allBtn);
    m.appendChild(head);

    // 列表
    const listBox = document.createElement("div");
    listBox.className = "import-list";
    for (const c of list) {
      const item = document.createElement("div");
      item.className = "import-item";
      const path = document.createElement("div");
      path.className = "import-path";
      path.textContent = c.path;
      path.title = c.path;
      const meta = document.createElement("div");
      meta.className = "import-meta";
      const badge = document.createElement("span");
      badge.className = `agent agent-${c.agent}`;
      badge.textContent = c.agent;
      const time = document.createElement("span");
      time.textContent = relTime(c.last_used);
      meta.append(badge, time);
      item.append(path, meta);
      item.onclick = async () => {
        if (imported.has(c.path.toLowerCase())) return;
        imported.add(c.path.toLowerCase());
        item.classList.add("imported");
        await this.store.importWs([c as ImportCandidate]).catch(() => {});
      };
      listBox.appendChild(item);
    }
    m.appendChild(listBox);

    this.showMenuAt(m, x, y);
  }

  /* ---------- 重命名 ---------- */

  private startRename(row: HTMLElement, ws: Workspace): void {
    const nameEl = row.querySelector<HTMLElement>(".ws-name");
    if (!nameEl) return;

    const input = document.createElement("input");
    input.className = "ws-rename";
    input.value = ws.name;
    nameEl.replaceWith(input);
    input.focus();
    input.select();

    let done = false;
    const finish = (commit: boolean) => {
      if (done) return;
      done = true;
      const v = input.value.trim();
      if (commit && v && v !== ws.name) {
        void this.store.rename(ws.id, v);
      } else {
        this.render();
      }
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

  private startFolderRename(header: HTMLElement, folder: Folder): void {
    const nameEl = header.querySelector<HTMLElement>(".folder-name");
    if (!nameEl) return;

    const input = document.createElement("input");
    input.className = "ws-rename";
    input.value = folder.name;
    nameEl.replaceWith(input);
    input.focus();
    input.select();

    let done = false;
    const finish = (commit: boolean) => {
      if (done) return;
      done = true;
      const v = input.value.trim();
      if (commit && v && v !== folder.name) {
        void this.store.folderRename(folder.id, v);
      } else {
        this.render();
      }
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

}

/* ---------- 菜单工具函数 ---------- */

export function closeAllMenus(): void {
  document.querySelectorAll(".context-menu").forEach((el) => el.remove());
}

function positionMenu(menu: HTMLElement, anchor: HTMLElement): void {
  const rect = anchor.getBoundingClientRect();
  const width = menu.offsetWidth || 200;
  menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - width - 8))}px`;
  menu.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - 60)}px`;
}

// 点击菜单外关闭
document.addEventListener("mousedown", (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest(".context-menu")) closeAllMenus();
});
