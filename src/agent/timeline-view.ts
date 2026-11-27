import type { AgentItem, AgentSession } from "./types";
import { commandDescription, commandText, previewLine, processOutcome } from "./activity";
import { itemHtml, recordDuration, stateLabel } from "./presentation";
import { identityItems, thinkingLive, timelineEntries, timelineItems } from "./timeline";

type CachedItem = { node: HTMLElement; signature: string };
function orderNodes(parent: HTMLElement, nodes: HTMLElement[]): void {
  let position = parent.firstElementChild;
  for (const node of nodes) {
    if (node !== position) parent.insertBefore(node, position);
    position = node.nextElementSibling;
  }
}
function preserveView(previous: HTMLElement, next: HTMLElement): void {
  const oldDetails = [...previous.querySelectorAll("details")], nextDetails = [...next.querySelectorAll("details")];
  if (previous instanceof HTMLDetailsElement && next instanceof HTMLDetailsElement) next.open = previous.open;
  oldDetails.forEach((detail, index) => { if (nextDetails[index]) nextDetails[index].open = detail.open; });
}
function renderItem(item: AgentItem, row: AgentSession, identity: boolean): HTMLElement {
  const process = ["tool", "thinking", "activity"].includes(item.kind);
  const thought = item.kind === "thinking", liveThought = thought && thinkingLive(item, row);
  const body = thought ? item.text?.trim() ? itemHtml(item, row) : "" : itemHtml(item, row);
  const detail = process && Boolean(body.trim());
  const card = document.createElement(detail ? "details" : "article");
  card.className = "agent-item agent-item-" + item.kind + (process ? " process-row" : "");
  card.dataset.item = item.id; card.dataset.status = thought && !liveThought && processOutcome(item) === "running" ? "success" : processOutcome(item);
  if (item.kind === "assistant") card.dataset.continuation = String(!identity);
  const label = document.createElement(detail ? "summary" : "div"); label.className = "agent-item-label";
  if (process) {
    const marker = document.createElement("span"); marker.className = "process-marker"; marker.setAttribute("aria-hidden", "true"); label.append(marker);
    const title = document.createElement("span"); title.className = "process-title";
    const command = commandText(item);
    title.textContent = thought ? liveThought ? "正在思考" : row.agent === "codex" ? "推理摘要" : "思考" : command ? "执行" : item.title || "执行步骤";
    label.append(title);
    const preview = thought ? previewLine(item.text || "") : command ? commandDescription(item) || previewLine(command) : "";
    if (preview) {
      const text = document.createElement("span"); text.className = "process-preview" + (command && !commandDescription(item) ? " process-command-preview" : "");
      text.textContent = preview.slice(0, 180); text.title = preview; label.append(text);
    }
    const status = document.createElement("small"); status.className = "agent-item-status";
    const outcome = card.dataset.status, duration = recordDuration(item.data), durationMs = item.data?.durationMs;
    const long = typeof durationMs === "number" ? durationMs >= 10000 : typeof item.data?.startedAtMs === "number" && typeof item.data?.completedAtMs === "number" && item.data.completedAtMs - item.data.startedAtMs >= 10000;
    status.textContent = outcome === "success" ? long ? duration : "" : stateLabel(item.status);
    if (outcome === "running" && ["running", "inProgress", "in_progress"].includes(item.status)) status.textContent = "";
    if (outcome === "failed" && typeof item.data?.exitCode === "number") status.textContent = "失败 · 退出码 " + item.data.exitCode;
    const semanticState=outcome==="failed"?status.textContent||"失败":stateLabel(thought&&!liveThought&&processOutcome(item)==="running"?"completed":item.status);
    status.setAttribute("aria-label",semanticState);
    label.append(status);
    label.setAttribute("aria-label",[title.textContent,preview,semanticState].filter(Boolean).join(" · "));
    if (detail) { const arrow = document.createElement("span"); arrow.className = "process-chevron"; arrow.textContent = "›"; arrow.setAttribute("aria-hidden", "true"); label.append(arrow); }
  } else {
    label.textContent = item.kind === "user" ? "你" : item.kind === "assistant" ? item.agentId ? "子 Agent · " + (row.tasks?.find(task => task.id === item.agentId)?.title || item.agentId) : row.agent === "claude" ? "Claude Code" : "Codex" : item.title;
    label.hidden = item.kind === "assistant" && !identity;
  }
  card.append(label);
  if (body) {
    const content = document.createElement("div"); content.className = "agent-content"; content.innerHTML = body;
    for (const link of content.querySelectorAll("a")) { link.target = "_blank"; link.rel = "noopener noreferrer"; }
    if (item.kind === "assistant" && ["running", "inProgress"].includes(item.status)) content.classList.add("message-running");
    card.append(content);
  }
  return card;
}

export class TimelineView {
  private items = new Map<string, CachedItem>();
  private groups = new Map<string, HTMLDetailsElement>();
  private scope = "";
  constructor(private root: HTMLElement) {}
  clear(): void { this.items.clear(); this.groups.clear(); this.scope = ""; this.root.replaceChildren(); }
  render(row: AgentSession, source: AgentItem[]): void {
    const scope=JSON.stringify([row.runtimeId,row.sessionId]);if(this.scope!==scope){this.clear();this.scope=scope;}
    const items = timelineItems(source, row), identities = identityItems(items), present = new Set(items.map(item => item.id));
    for (const [id, cached] of this.items) if (!present.has(id)) { cached.node.remove(); this.items.delete(id); }
    for (const item of items) {
      const identity = identities.has(item.id), live = item.kind === "thinking" && thinkingLive(item, row);
      const signature = JSON.stringify([item, identity, live, row.agent, item.agentId ? row.tasks?.find(task => task.id === item.agentId)?.title : ""]), old = this.items.get(item.id);
      if (old?.signature === signature) continue;
      const node = renderItem(item, row, identity);
      if (old) {
        const focused = document.activeElement, headerFocus = focused === old.node.querySelector("summary");
        const content=old.node.querySelector(".agent-content"),scrollTop=content?.scrollTop??0,scrollLeft=content?.scrollLeft??0;
        preserveView(old.node, node); old.node.replaceWith(node);
        const nextContent=node.querySelector(".agent-content");if(nextContent){nextContent.scrollTop=scrollTop;nextContent.scrollLeft=scrollLeft;}
        if (headerFocus) node.querySelector("summary")?.focus({ preventScroll: true });
      }
      this.items.set(item.id, { node, signature });
    }
    const focused=document.activeElement;
    const entries = timelineEntries(items), nodes: HTMLElement[] = [], groups = new Set<string>();
    for (const entry of entries) {
      const children = entry.items.map(item => this.items.get(item.id)!.node);
      if (children.length === 1) { nodes.push(children[0]); continue; }
      groups.add(entry.key);
      let group = this.groups.get(entry.key);
      if (!group) {
        group = document.createElement("details"); group.className = "command-group process-row";
        group.dataset.group = entry.key; group.dataset.status = "success";
        const heading = document.createElement("summary"); heading.className = "agent-item-label";
        const marker = document.createElement("span"); marker.className = "process-marker"; marker.setAttribute("aria-hidden", "true");
        const title = document.createElement("span"); title.className = "process-title";
        const arrow = document.createElement("span"); arrow.className = "process-chevron"; arrow.textContent = "›"; arrow.setAttribute("aria-hidden", "true");
        heading.append(marker, title, arrow);
        const body = document.createElement("div"); body.className = "command-group-body"; group.append(heading, body);
        group.open = children.some(child => child instanceof HTMLDetailsElement && child.open || child.contains(document.activeElement));
        this.groups.set(entry.key, group);
      }
      group.querySelector(".process-title")!.textContent = "执行了 " + children.length + " 条命令";
      const body = group.querySelector<HTMLElement>(".command-group-body")!;
      orderNodes(body, children);
      for (const child of [...body.children]) if (!children.includes(child as HTMLElement)) child.remove();
      nodes.push(group);
    }
    orderNodes(this.root, nodes);
    for (const [key, group] of this.groups) if (!groups.has(key)) { group.remove(); this.groups.delete(key); }
    if(focused instanceof HTMLElement&&this.root.contains(focused)&&document.activeElement!==focused)focused.focus({preventScroll:true});
  }
}
