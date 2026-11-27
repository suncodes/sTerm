export interface AgentItem { id: string; kind: string; title: string; text: string; status: string; turnId: string }
export interface AgentApproval { id: string; title: string; detail: string; turnId: string }
export interface AgentSession {
  protocolVersion: number;
  runtimeId: string; sessionId: string; tabId: number; generation: number; agent: "claude" | "codex";
  title: string; cwd: string; model: string; models: string[]; policy: string; state: string;
  turnId: string; seq: number; settingsRevision: number; shared: boolean; allowControl: boolean; error: string | null;
  capabilities: { send: boolean; stop: boolean; models: boolean; plan: boolean; policy: boolean; approvals: boolean };
  items: AgentItem[]; approvals: AgentApproval[];
}
export interface AgentRoster { revision: number; sessions: AgentSession[] }
export type AgentAction = { action: "sendText"; text: string } | { action: "stopTurn"; turnId: string } |
  { action: "setModel"; model: string } | { action: "setPolicy"; sandbox: string; approvalPolicy: string } | { action: "enterPlan" } |
  { action: "respondApproval"; requestId: string; allow: boolean };

export type AgentEvent =
  | { type: "ready"; sessionId: string; model: string; models: string[]; capabilities: AgentSession["capabilities"] }
  | { type: "state"; state: string; turnId: string }
  | { type: "items"; items: AgentItem[] }
  | { type: "upsert"; item: AgentItem }
  | { type: "delta"; id: string; delta: string }
  | { type: "settings"; model?: string | null; policy?: string | null }
  | { type: "approval"; approval: AgentApproval }
  | { type: "resolved"; id: string }
  | { type: "error"; message: string }
  | { type: "result"; id: string; ok: boolean; message?: string | null };
export interface AgentChange { seq: number; events: AgentEvent[] }
