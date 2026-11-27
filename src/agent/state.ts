import type { AgentChange, AgentSession } from "./types";

/** 同一游标重复到达不会重复追加；缺口要求重新获取快照。 */
export function applyChanges(session: AgentSession, changes: AgentChange[]): AgentSession | null {
  let row = { ...session, items: [...session.items], approvals: [...session.approvals] };
  for (const change of changes) {
    if (change.seq <= row.seq) continue;
    if (change.seq !== row.seq + 1) return null;
    for (const event of change.events) {
      switch (event.type) {
        case "ready":
          if (event.sessionId !== row.sessionId) { row.items = []; row.approvals = []; row.turnId = ""; }
          row.sessionId = event.sessionId; row.model = event.model; row.models = event.models;
          row.capabilities = event.capabilities; row.state = "idle"; row.error = null; row.settingsRevision++; break;
        case "state":
          if (!["idle","running","waiting","offline","exited"].includes(event.state)) break;
          row.state = event.state; row.turnId = event.turnId;
          if (event.state === "running") row.error = null;
          if (["idle", "offline", "exited"].includes(row.state)) row.approvals = [];
          break;
        case "items": row.items = event.items; break;
        case "upsert": {
          const index = row.items.findIndex(item => item.id === event.item.id);
          if (index < 0) row.items.push(event.item); else row.items[index] = event.item;
          break;
        }
        case "delta": row.items = row.items.map(item => item.id === event.id ? { ...item, text: (item.text + event.delta).slice(0, 65536) } : item); break;
        case "settings":
          if (event.model != null) row.model = event.model;
          if (event.policy != null) row.policy = event.policy;
          row.settingsRevision++;
          break;
        case "approval":
          if (!row.approvals.some(item => item.id === event.approval.id)) row.approvals.push(event.approval);
          row.state = "waiting"; break;
        case "resolved":
          row.approvals = row.approvals.filter(item => item.id !== event.id);
          if (!row.approvals.length && row.state === "waiting") row.state = "running";
          break;
        case "error": row.error = event.message; break;
        case "result": break;
      }
    }
    row.seq = change.seq;
  }
  return row;
}
