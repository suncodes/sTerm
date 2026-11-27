import type { AgentNotice, AgentSession } from "./types";

export const noticeKey = (entry: AgentNotice): string => JSON.stringify([entry.agentId || "", entry.id]);
export const noticeFingerprint = (entry: AgentNotice): string => JSON.stringify([entry.title, entry.message, entry.severity, entry.state, entry.phase]);
export const importantNotice = (entry: AgentNotice): boolean => ["warning", "error"].includes(entry.severity || "");
export function activeNotices(row: AgentSession): AgentNotice[] {
  return (row.notices ?? []).filter(entry => {
    if (entry.phase !== "running") return false;
    if (entry.agentId) return (row.tasks ?? []).some(task => task.id === entry.agentId && ["running", "waiting", "pending", "retrying"].includes(task.state));
    if (!["running", "waiting", "retrying"].includes(row.state)) return false;
    return !entry.turnId || !row.turnId || entry.turnId === row.turnId;
  });
}

export class NoticeTracker {
  private sessions = new Map<string, Map<string, string>>();
  update(row: AgentSession): AgentNotice[] {
    const key = JSON.stringify([row.runtimeId, row.sessionId]);
    const previous = this.sessions.get(key), current = new Map<string, string>(), fresh: AgentNotice[] = [];
    for (const entry of row.notices ?? []) {
      const identity = noticeKey(entry), fingerprint = noticeFingerprint(entry);
      current.set(identity, fingerprint);
      if (previous && previous.get(identity) !== fingerprint) fresh.push(entry);
    }
    this.sessions.set(key, current);
    if (this.sessions.size > 64) this.sessions.delete(this.sessions.keys().next().value!);
    return fresh;
  }
}
