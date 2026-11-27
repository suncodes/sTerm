import type { AgentChange, AgentSession } from "./types";

export function applyChanges(session: AgentSession, changes: AgentChange[]): AgentSession | null {
  const row = { ...session, items: [...session.items], approvals: [...session.approvals],
    interactions: [...(session.interactions ?? [])], tasks: [...(session.tasks ?? [])], queuedInputs: [...(session.queuedInputs ?? [])] };
  for (const change of changes) {
    if (change.seq <= row.seq) continue;
    if (change.seq !== row.seq + 1) return null;
    for (const event of change.events) {
      switch (event.type) {
        case "ready":
          if (event.sessionId !== row.sessionId) {
            row.permissions = {}; row.items = []; row.approvals = []; row.interactions = []; row.tasks = []; row.queuedInputs = []; row.activity = {}; row.turnId = "";
          }
          row.sessionId = event.sessionId; row.model = event.model; row.models = event.models;
          row.capabilities = { ...event.capabilities, end: row.capabilities.end }; row.state = "idle"; row.error = null; row.settingsRevision++; break;
        case "state":
          if (!["idle","running","waiting","offline","exited","retrying","failed","interrupted","blocked"].includes(event.state)) break;
          row.state = event.state==="running"&&row.interactions.some(i=>i.blocking&&!i.agentId)?"waiting":event.state; row.turnId = event.turnId;
          if (event.state === "running") row.error = null;
          if (["idle", "offline", "exited"].includes(row.state)) row.approvals = [];
          break;
        case "items": row.items = [...row.items.filter(item => item.agentId), ...event.items]; break;
        case "taskItems": row.items = [...row.items.filter(item => item.agentId !== event.agentId), ...event.items]; break;
        case "upsert": {
          const index = row.items.findIndex(item => item.id === event.item.id);
          if (index < 0) row.items.push(event.item); else row.items[index] = event.item;
          break;
        }
        case "delta": row.items = row.items.map(item => item.id === event.id ? { ...item, text: item.text + event.delta } : item); break;
        case "settings":
          if (event.model != null) row.model = event.model;
          if (event.policy != null) row.policy = event.policy;
          row.settingsRevision++; break;
        case "permissions":
          if (JSON.stringify(row.permissions) !== JSON.stringify(event.value)) { row.permissions = event.value; row.settingsRevision++; } break;
        case "queue": row.queuedInputs = event.items.slice(0, 100); break;
        case "activity": row.activity = event.value; break;
        case "task": {
          const index = row.tasks.findIndex(task => task.id === event.task.id);
          if (index < 0) { if (row.tasks.length < 64) row.tasks.push(event.task); } else row.tasks[index] = event.task; break;
        }
        case "interaction": {
          const index = row.interactions.findIndex(item => item.id === event.interaction.id);
          if (index < 0) { if (row.interactions.length < 64) row.interactions.push(event.interaction); } else row.interactions[index] = event.interaction;
          if (row.interactions.some(item => item.blocking && !item.agentId) && !["offline","exited","blocked"].includes(row.state)) row.state = "waiting";
          break;
        }
        case "approval":
          if (!row.approvals.some(item => item.id === event.approval.id)) row.approvals.push(event.approval);
          row.state = "waiting"; break;
        case "resolved":
          row.approvals = row.approvals.filter(item => item.id !== event.id);
          row.interactions = row.interactions.filter(item => item.id !== event.id);
          if (!row.approvals.length && !row.interactions.some(item => item.blocking && !item.agentId) && row.state === "waiting") row.state = "running";
          break;
        case "error": row.error = event.message; break;
        case "result": break;
      }
    }
    row.seq = change.seq;
  }
  return row;
}
