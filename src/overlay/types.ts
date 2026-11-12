import type { AgentName } from "../types";
import type { AgentStatus } from "../terminal/agentstate";

export interface OverlaySession {
  id: number;
  title: string;
  agent: AgentName;
  status: Exclude<AgentStatus, "none">;
  active: boolean;
}

export interface OverlaySnapshot {
  enabled: boolean;
  background_opacity: number;
  revision: number;
  sessions: OverlaySession[];
}

/** 与标签共用同一状态；普通 Shell 不占用悬浮窗列表。 */
export function collectAgentSessions(
  sessions: readonly {
    id: number;
    title: string;
    agentState: { agent: AgentName | null; status: AgentStatus };
  }[],
  activeId?: number,
): OverlaySession[] {
  return sessions.flatMap((s) => {
    const { agent, status } = s.agentState;
    return agent && status !== "none"
      ? [{ id: s.id, title: s.title, agent, status, active: s.id === activeId }]
      : [];
  });
}
