import type { AgentItem, AgentSession } from "./types";
import { runningStates } from "./presentation";
import { successfulCommand } from "./activity";

export interface TimelineEntry { key: string; items: AgentItem[] }
export function thinkingLive(item: AgentItem, row: AgentSession): boolean {
  if (!runningStates.includes(item.status)) return false;
  if (item.agentId) {
    const task = row.tasks?.find(task => task.id === item.agentId);
    return Boolean(task && runningStates.includes(task.state) && (!item.turnId || !task.turnId || task.turnId === item.turnId));
  }
  return ["running", "waiting", "retrying"].includes(row.state) && (!item.turnId || item.turnId === row.turnId);
}
export function timelineItems(items: AgentItem[], row: AgentSession): AgentItem[] {
  return items.filter(item => item.kind !== "thinking" || Boolean(item.text?.trim()) || thinkingLive(item, row));
}
export function timelineEntries(items: AgentItem[]): TimelineEntry[] {
  const entries: TimelineEntry[] = [];
  for (const item of items) {
    const previous = entries[entries.length - 1], last = previous?.items[previous.items.length - 1];
    // 只将相邻、同回合的成功命令分组；活动、失败和正文都是可见边界。
    if (last && item.turnId && item.turnId === last.turnId && (item.agentId || "") === (last.agentId || "") && successfulCommand(item) && successfulCommand(last)) {
      previous.items.push(item);
    } else entries.push({ key: item.id, items: [item] });
  }
  return entries;
}
export function identityItems(items: AgentItem[]): Set<string> {
  const first = new Set<string>();
  let identity = "";
  for (const item of items) {
    if (item.kind === "user") { identity = ""; continue; }
    if (item.kind !== "assistant") continue;
    const key = JSON.stringify([item.agentId || "", item.turnId || ""]);
    if (identity !== key) first.add(item.id);
    identity = key;
  }
  return first;
}
