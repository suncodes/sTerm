import type { AgentName } from "../types";
import type { AgentEventKind } from "./agentevents";

/** Agent 会话状态：无 / 运行中 / 空闲（任务完成）/ 已退出 */
export type AgentStatus = "none" | "running" | "idle" | "exited";

const EXIT_LINGER_MS = 5000;

/** 文件信箱与终端 OSC 共用的事件状态机，不根据输出文字猜测进度。 */
export class AgentState {
  status: AgentStatus = "none";
  agent: AgentName | null = null;
  private exitedAt = 0;
  private timer: number | undefined;

  constructor(private readonly onChange: () => void) {
    this.timer = window.setInterval(() => this.tick(), 1000);
  }

  reset(): void {
    this.exitedAt = 0;
    this.set("none", null);
    if (this.timer === undefined) this.timer = window.setInterval(() => this.tick(), 1000);
  }

  markAgent(name: AgentName): void {
    this.set("running", name);
  }

  markAgentEvent(name: AgentName, event: AgentEventKind): void {
    switch (event) {
      case "start":
      case "stop":
        this.set("idle", name);
        break;
      case "prompt":
        this.set("running", name);
        break;
      case "end":
        this.set("none", null);
        break;
    }
  }

  /** shell 提示符出现，说明已经退回 shell。 */
  markShellPrompt(): void {
    if (this.status === "running" || this.status === "idle") this.set("none", null);
  }

  markExited(): void {
    if (this.agent) {
      this.exitedAt = Date.now();
      this.set("exited");
    } else {
      this.dispose();
    }
  }

  dispose(): void {
    if (this.timer !== undefined) window.clearInterval(this.timer);
    this.timer = undefined;
    this.set("none", null);
  }

  private tick(): void {
    if (this.status === "exited" && this.exitedAt && Date.now() - this.exitedAt > EXIT_LINGER_MS) {
      this.dispose();
    }
  }

  private set(status: AgentStatus, agent: AgentName | null = this.agent): void {
    if (this.status === status && this.agent === agent) return;
    this.status = status;
    this.agent = agent;
    this.onChange();
  }
}
