import type { AgentChange, AgentSession } from "./types";

export function applyChanges(session: AgentSession, changes: AgentChange[]): AgentSession | null {
  const row = { ...session, items: [...session.items], approvals: [...session.approvals],
    interactions: [...(session.interactions ?? [])], tasks: [...(session.tasks ?? [])], queuedInputs: [...(session.queuedInputs ?? [])], turns:[...(session.turns??[])],notices:[...(session.notices??[])] };
  for (const change of changes) {
    if (change.seq <= row.seq) continue;
    if (change.seq !== row.seq + 1) return null;
    for (const event of change.events) {
      switch (event.type) {
        case "ready":
          if (event.sessionId !== row.sessionId) {
            row.permissions = {}; row.items = []; row.approvals = []; row.interactions = []; row.tasks = []; row.turns=[];row.notices=[];row.metadata={}; row.queuedInputs = []; row.activity = {}; row.turnId = "";
          }
          row.sessionId = event.sessionId; row.model = event.model; row.models = event.models;
          row.capabilities = { ...event.capabilities, end: row.capabilities.end }; row.state = row.activity?.kind==="blocked"?"blocked":"idle"; row.error = null; row.settingsRevision++; break;
        case "state":
          if (!["idle","running","waiting","offline","exited","retrying","failed","interrupted","blocked"].includes(event.state)) break;
          row.state = event.state==="running"&&row.interactions.some(i=>i.blocking&&!i.agentId)?"waiting":event.state; row.turnId = event.turnId;
          if (event.state === "running") row.error = null;
          if (["idle", "offline", "exited"].includes(row.state)) row.approvals = [];
          if(["offline","exited"].includes(row.state))row.tasks=row.tasks.map(task=>["running","inProgress","in_progress","waiting","pending","retrying"].includes(task.state)?{...task,state:"unknown",data:{...task.data,statusReason:"连接已断开，等待原生状态确认"}}:task);
          break;
        case "items": row.items = [...row.items.filter(item => item.agentId), ...event.items]; break;
        case "taskItems": row.items = [...row.items.filter(item => item.agentId !== event.agentId), ...event.items]; break;
        case "upsert": {
          const index = row.items.findIndex(item => item.id === event.item.id);
          if (index < 0) row.items.push(event.item); else row.items[index] = {...event.item,data:mergeData(row.items[index].data,event.item.data)};
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
        case "metadata": row.metadata={...row.metadata,...event.value};if(typeof event.value.title==="string")row.title=event.value.title;if(typeof event.value.cwd==="string")row.cwd=event.value.cwd;break;
        case "turn": {
          const index=row.turns.findIndex(turn=>turn.id===event.value.id&&turn.agentId===event.value.agentId);
          if(index<0){row.turns.push(event.value);if(row.turns.length>128)row.turns.shift();}
          else row.turns[index]=mergeData(row.turns[index],event.value) as unknown as typeof event.value;
          break;
        }
        case "notice": {
          const index=row.notices.findIndex(notice=>notice.id===event.value.id&&notice.agentId===event.value.agentId);
          if(index<0){row.notices.push(event.value);if(row.notices.length>32)row.notices.shift();}else row.notices[index]={...row.notices[index],...event.value};break;
        }
        case "task": {
          const index = row.tasks.findIndex(task => task.id === event.task.id);
          if (index < 0) { if(row.tasks.length>=64){const terminal=row.tasks.findIndex(task=>["completed","failed","error","declined","stopped","killed","interrupted"].includes(task.state));if(terminal>=0)row.tasks.splice(terminal,1);}if (row.tasks.length < 64) row.tasks.push(event.task); } else row.tasks[index] = {...event.task,kind:event.task.kind||row.tasks[index].kind,data:mergeData(row.tasks[index].data,event.task.data)}; break;
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

function mergeData(previous:Record<string,unknown>|undefined,next:Record<string,unknown>|undefined):Record<string,unknown>{
  const value={...previous};for(const [key,entry] of Object.entries(next??{})){if(entry==null&&(key.endsWith("AtMs")||key==="durationMs"))continue;value[key]=entry;}return value;
}
