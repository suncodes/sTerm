import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { normPath } from "../util";
import type { AgentSession, Folder, ImportCandidate, StoreData, Workspace } from "../types";

/** 工作区前端仓库：封装 Rust 命令 + 监听同步事件 */
export class WorkspaceStore {
  data: StoreData = { folders: [], workspaces: [] };
  private listeners = new Set<() => void>();
  private unlisten: UnlistenFn | null = null;

  async init(): Promise<void> {
    this.data = await invoke<StoreData>("store_list");
    this.unlisten = await listen<StoreData>("workspaces-changed", (e) => {
      this.data = e.payload;
      this.emitChange();
    });
  }

  /* ---------- 查询 ---------- */

  hasPath(path: string): boolean {
    const n = normPath(path);
    return this.data.workspaces.some((w) => normPath(w.path) === n);
  }

  hasAgent(path: string, agent: string): boolean {
    const n = normPath(path);
    return this.data.workspaces.some(
      (w) => normPath(w.path) === n && w.agents.includes(agent),
    );
  }

  /* ---------- workspace ---------- */

  add(path: string, agent?: string): Promise<Workspace> {
    return invoke<Workspace>("ws_add", { path, agent });
  }

  rename(id: string, name: string): Promise<void> {
    return invoke("ws_rename", { id, name });
  }

  remove(id: string): Promise<void> {
    return invoke("ws_remove", { id });
  }

  pin(id: string, pinned: boolean): Promise<void> {
    return invoke("ws_pin", { id, pinned });
  }

  move(id: string, folderId: string | null): Promise<void> {
    return invoke("ws_move", { id, folderId });
  }

  /** 合并 agent 徽标（不动 last_used）：展开会话列表后用实际扫描结果校正卡片 */
  syncAgents(path: string, agents: string[]): Promise<void> {
    return invoke("ws_sync_agents", { path, agents });
  }

  /* ---------- folder ---------- */

  folderAdd(name: string, parentId?: string | null): Promise<Folder> {
    return invoke<Folder>("folder_add", { name, parentId: parentId ?? null });
  }

  /** 移动文件夹：改父级（嵌套）或排序；beforeId 为 null 时追加到同父级末尾 */
  folderMove(id: string, parentId: string | null, beforeId: string | null): Promise<void> {
    return invoke("folder_move", { id, parentId, beforeId });
  }

  folderRename(id: string, name: string): Promise<void> {
    return invoke("folder_rename", { id, name });
  }

  folderRemove(id: string): Promise<void> {
    return invoke("folder_remove", { id });
  }

  folderToggle(id: string, collapsed: boolean): Promise<void> {
    return invoke("folder_toggle", { id, collapsed });
  }

  /* ---------- agent 扫描 / 导入 ---------- */

  importScan(): Promise<ImportCandidate[]> {
    return invoke("agent_import_scan");
  }

  importWs(candidates: ImportCandidate[]): Promise<number> {
    return invoke("ws_import", { candidates });
  }

  listSessions(path: string, limit = 5): Promise<AgentSession[]> {
    return invoke("agent_sessions", { path, limit });
  }

  /* ---------- 订阅 ---------- */

  onChange(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emitChange(): void {
    for (const f of this.listeners) f();
  }

  dispose(): void {
    this.unlisten?.();
    this.unlisten = null;
  }
}
