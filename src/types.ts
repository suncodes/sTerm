/** Rust 侧 list_shells 返回的 shell 描述 */
export interface ShellInfo {
  id: string;
  name: string;
  program: string;
  args: string[];
}

/** 工作区分组文件夹（支持多级嵌套，parent_id 为 null 表示根级） */
export interface Folder {
  id: string;
  name: string;
  collapsed: boolean;
  parent_id: string | null;
}

/** Rust 侧 workspaces.json 中的工作区条目（字段保持 snake_case 与 Rust 一致） */
export interface Workspace {
  id: string;
  name: string;
  path: string;
  agents: string[];
  pinned: boolean;
  created: number;
  last_used: number;
  folder_id: string | null;
}

/** Rust 侧 Store::list() 返回的完整数据 */
export interface StoreData {
  folders: Folder[];
  workspaces: Workspace[];
}

/** M2 导入扫描结果 */
export interface ImportCandidate {
  agent: string;
  path: string;
  last_used: number;
}

/** M3 工作区下的会话条目 */
export interface AgentSession {
  /** 会话标题（首条用户消息摘要） */
  title?: string | null;
  agent: string;
  session_id: string;
  last_used: number;
}

export type AgentName = "claude" | "codex" | "pi";
