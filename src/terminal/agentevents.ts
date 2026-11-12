import type { AgentName } from "../types";

export type AgentEventKind = "start" | "prompt" | "stop" | "end";

export interface AgentEvent {
  agent: AgentName;
  event: AgentEventKind;
}

export interface AgentOscEvent extends AgentEvent {
  session: string;
}

type NativeAgent = "codex" | "claude";
type ProgramState = "idle" | "working" | "done" | "blocked" | "error" | "clear";
interface ProgramStatus {
  agent: NativeAgent | null;
  state: ProgramState;
  id: string | null;
}

const EVENTS = new Set(["start", "prompt", "stop", "end"]);
const PROGRAM_STATES = new Set(["idle", "working", "done", "blocked", "error", "clear"]);
// 最小能力响应；Claude 收到响应后才会发送 OSC 7501 状态。
export const PROGRAM_STATUS_RESPONSE = "\x1b]7501;?\x1b\\";

/** OSC 7501 字段无固定顺序；附加说明不参与状态判断，也不展示其内容。 */
export function parseProgramStatus(data: string): ProgramStatus | null {
  if (!data || data.length > 4087 || /[^\x20-\x7e]/.test(data)) return null;
  const fields = new Map<string, string>();
  for (const field of data.split(":")) {
    const index = field.indexOf("=");
    if (index <= 0) return null;
    const key = field.slice(0, index);
    if (!/^[a-z][a-z0-9_-]{0,15}$/.test(key) || fields.has(key)) return null;
    fields.set(key, field.slice(index + 1));
  }
  const state = fields.get("state") ?? "";
  if (!PROGRAM_STATES.has(state)) return null;
  const app = fields.get("app");
  const agent = app === "codex" ? "codex" : app === "claude-code" ? "claude" : null;
  if ((app !== undefined && !agent) || (state !== "clear" && !agent)) return null;
  const id = fields.get("id") ?? null;
  if (id !== null && (id.length > 128 || !/^[A-Za-z0-9_+.-]{1,32}(\/[A-Za-z0-9_+.-]{1,32}){0,7}$/.test(id))) return null;
  return { agent, state: state as ProgramState, id };
}

/** 文件信箱仅接收 Pi 扩展；Claude、Codex 的旧 Hook 不再驱动状态。 */
export function parseAgentKind(kind: string): AgentEvent | null {
  const [agent, event, extra] = kind.split("-");
  if (extra !== undefined || agent !== "pi" || !EVENTS.has(event)) return null;
  return { agent, event: event as AgentEventKind };
}

/** Pi 扩展：OSC 777;sterm;v=1;agent=pi;event=...;session=...。 */
export function parseAgentOsc(data: string): AgentOscEvent | null {
  if (!data.startsWith("sterm;") || data.length > 512) return null;
  const fields = new Map<string, string>();
  for (const field of data.slice(6).split(";")) {
    const index = field.indexOf("=");
    if (index <= 0) return null;
    const key = field.slice(0, index);
    if (fields.has(key) || !["v", "agent", "event", "session"].includes(key)) return null;
    fields.set(key, field.slice(index + 1));
  }
  if (fields.size !== 4 || fields.get("v") !== "1" || fields.get("agent") !== "pi") return null;
  const event = fields.get("event") ?? "";
  const session = fields.get("session") ?? "";
  if (!EVENTS.has(event) || !/^[A-Za-z0-9._:-]{1,128}$/.test(session)) return null;
  return { agent: "pi", event: event as AgentEventKind, session };
}

/** 每个终端独立绑定原生主会话或 Pi 扩展会话，丢弃被替代会话的迟到事件。 */
export class AgentOscChannel {
  private current: string | null = null;
  private retired = new Set<string>();

  constructor(private readonly deliver: (event: AgentOscEvent) => void) {}

  /** OSC 会话存续期间忽略较慢的本地文件通知，结束后恢复。 */
  get hasControl(): boolean {
    return this.current !== null;
  }

  accept(data: string): boolean {
    if (!data.startsWith("sterm;")) return false;
    const event = parseAgentOsc(data);
    if (!event) return true;
    const key = event.agent + "/" + event.session;
    if (event.event === "start") {
      if (this.current && this.current !== key) this.retire(this.current);
      // resume 可能沿用同一个会话 ID；明确的 start 可以重新绑定。
      this.retired.delete(key);
      this.current = key;
    } else {
      if (this.retired.has(key)) return true;
      if (event.event === "prompt" && this.current === null) this.current = key;
      if (this.current !== key) return true;
    }
    if (event.event === "end") {
      this.retire(key);
      this.current = null;
    }
    this.deliver(event);
    return true;
  }

  /** Codex 与 Claude 共用主会话状态；带 id 的后台任务不覆盖或清除主会话。 */
  acceptProgramStatus(data: string): boolean {
    const report = parseProgramStatus(data);
    if (!report) return false;
    if (report.id !== null) return true;
    const currentAgent = this.current === "native:codex" ? "codex"
      : this.current === "native:claude" ? "claude" : null;
    const agent = report.agent ?? currentAgent;
    if (!agent) return true;
    if (report.state === "clear") {
      if (agent !== currentAgent) return true;
      this.current = null;
    } else {
      const key = "native:" + agent;
      if (this.current && this.current !== key) this.retire(this.current);
      this.current = key;
    }
    this.deliver({
      agent,
      event: report.state === "clear" ? "end" : report.state === "working" ? "prompt" : "stop",
      session: "terminal",
    });
    return true;
  }

  shellPrompt(): void {
    if (this.current) this.retire(this.current);
    this.current = null;
  }

  reset(): void {
    this.current = null;
    this.retired.clear();
  }

  private retire(key: string): void {
    this.retired.add(key);
    if (this.retired.size > 32) this.retired.delete(this.retired.values().next().value!);
  }
}
