import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { OverlayKeepAlive } from "../overlay/keepalive";
import type { AgentEvent } from "../terminal/agentevents";
import type { TerminalSession } from "../terminal/session";
import type { TabManager } from "../terminal/tabmanager";

export interface FeishuStatus {
  enabled: boolean;
  connected: boolean;
  connectionError?: string | null;
  deliveryError?: string | null;
}

export interface FeishuSettings {
  status: FeishuStatus;
  appId: string;
  hasSecret: boolean;
  chatId: string;
  allowedUsers: string[];
  allowAllUsers: boolean;
  sendTitle: boolean;
}

interface FeishuInput {
  request: string;
  id: number;
  generation: number;
}

export class FeishuNotifications {
  private readonly keepAlive = new OverlayKeepAlive();
  private readonly reports = new Map<number, Promise<void>>();

  async init(tabs: TabManager): Promise<void> {
    await listen<FeishuStatus>("feishu-status", event => this.keepAlive.setEnabled(event.payload.enabled));
    await listen<FeishuInput>("feishu-input", ({ payload }) => {
      const { request, id, generation } = payload;
      const session = tabs.byId(id);
      // 先同步已经解析出的状态，再交由 Rust 在写入时校验进程代次和输入权限。
      void (async () => {
        const ready = await session?.allowsAgentInput(generation) ?? false;
        await this.reports.get(id);
        await invoke("feishu_input_result", { request, ready, paste: session?.term.modes.bracketedPasteMode ?? false });
      })().catch(() => console.warn("飞书续问确认失败"));
    });
    const settings = await invoke<FeishuSettings>("feishu_settings");
    this.keepAlive.setEnabled(settings.status.enabled);
    window.addEventListener("beforeunload", () => this.keepAlive.setEnabled(false), { once: true });
  }

  report(session: TerminalSession, event: AgentEvent): void {
    const { id, generation, title } = session.remoteInfo();
    const report = {
      id, generation, title, agent: event.agent, event: event.event,
    };
    const operation = Promise.resolve(this.reports.get(id))
      .then(() => invoke<void>("feishu_agent_event", { report }))
      .catch(() => console.warn("Agent 事件同步失败"))
      .finally(() => { if (this.reports.get(id) === operation) this.reports.delete(id); });
    this.reports.set(id, operation);
  }
}
