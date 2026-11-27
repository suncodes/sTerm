export interface AgentItem { id: string; kind: string; title: string; text: string; status: string; turnId: string; agentId?: string; data?: Record<string, unknown> }
export interface AgentApproval { id: string; title: string; detail: string; turnId: string }
export interface AgentOption { label: string; description?: string; preview?: string }
export interface AgentQuestion { id: string; title: string; header?: string; multiSelect?: boolean; secret?: boolean; other?: boolean; preview?: string; options?: AgentOption[] | null }
export interface AgentInteraction {
  id: string; kind: string; title: string; detail: string; turnId: string; agentId: string; blocking: boolean;
  data: { questions?: AgentQuestion[]; cancellable?: boolean; decisions?: { label: string; value: unknown }[];
    schema?: Record<string, unknown>; url?: string; permissions?: unknown; [key: string]: unknown };
}
export interface AgentTask { id: string; parentId: string; title: string; model: string; state: string; turnId: string; message: string; kind?: string; data?: Record<string, unknown> }
export interface AgentTurn { id: string; agentId?: string; state: string; startedAtMs?: number | null; completedAtMs?: number | null; durationMs?: number | null; timeSource?: string; usage?: Record<string, unknown>; [key: string]: unknown }
export interface AgentNotice { id: string; message?: string; title?: string; severity?: string; agentId?: string; [key: string]: unknown }
export interface AgentPermission { profile: string; label: string; description?: string; approvalPolicy: string; reviewer: string; allowed: boolean }
export interface AgentBlockDetails { errorType?: string | null; detailedExplanation?: string | null; steer?: { message?: string | null } | null; reviewTarget?: string | null }
export interface AgentSession {
  protocolVersion: number;
  runtimeId: string; sessionId: string; tabId: number; generation: number; agent: "claude" | "codex";
  title: string; cwd: string; model: string; models: string[]; policy: string; state: string;
  turnId: string; seq: number; settingsRevision: number; shared: boolean; allowControl: boolean; error: string | null;
  capabilities: { send: boolean; stop: boolean; models: boolean; plan: boolean; policy: boolean; approvals: boolean; end?: boolean; queue?: boolean };
  items: AgentItem[]; approvals: AgentApproval[]; interactions: AgentInteraction[]; tasks: AgentTask[];
  permissions: { current?: { profile: string; approvalPolicy: string; reviewer: string }; options?: AgentPermission[] };
  queuedInputs: { id: string; text: string }[]; activity: { kind?: string; message?: string; details?: AgentBlockDetails | null; turnId?: string; reviewId?: string | null; continuationStatus?: string };
  turns?: AgentTurn[]; notices?: AgentNotice[]; metadata?: Record<string, unknown>; clockMs?: number;
}
export interface AgentRoster { revision: number; sessions: AgentSession[] }
export type AgentAction = { action: "sendText"; text: string } | { action: "stopTurn"; turnId: string } |
  { action: "setModel"; model: string } | { action: "setPolicy"; sandbox: string; approvalPolicy: string } | { action: "enterPlan" } |
  { action: "setPermissions"; profile: string; approvalPolicy: string; reviewer: string } |
  { action: "continueBlocked"; turnId: string; reviewId: string } |
  { action: "respondApproval"; requestId: string; allow: boolean } |
  { action: "respondInteraction"; requestId: string; response: unknown } |
  { action: "queue"; operation: "add" | "update" | "delete" | "reorder" | "start"; queueId?: string; text?: string; ids?: string[] };

export type AgentEvent =
  | { type: "ready"; sessionId: string; model: string; models: string[]; capabilities: AgentSession["capabilities"] }
  | { type: "state"; state: string; turnId: string }
  | { type: "items"; items: AgentItem[] }
  | { type: "taskItems"; agentId: string; items: AgentItem[] }
  | { type: "upsert"; item: AgentItem }
  | { type: "delta"; id: string; delta: string }
  | { type: "settings"; model?: string | null; policy?: string | null }
  | { type: "permissions"; value: AgentSession["permissions"] }
  | { type: "queue"; items: AgentSession["queuedInputs"] }
  | { type: "activity"; value: AgentSession["activity"] }
  | { type: "turn"; value: AgentTurn }
  | { type: "notice"; value: AgentNotice }
  | { type: "metadata"; value: Record<string, unknown> }
  | { type: "task"; task: AgentTask }
  | { type: "interaction"; interaction: AgentInteraction }
  | { type: "approval"; approval: AgentApproval }
  | { type: "resolved"; id: string }
  | { type: "error"; message: string }
  | { type: "result"; id: string; ok: boolean; message?: string | null };
export interface AgentChange { seq: number; events: AgentEvent[] }
