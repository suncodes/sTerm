import "./style.css";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { RemoteLogin, pairRemoteBrowser } from "../remote/login";
import { applyChanges } from "./state";
import type { AgentChange, AgentAction, AgentItem, AgentRoster, AgentSession } from "./types";

const byId = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const list = byId("agent-sessions"), messages = byId("agent-messages"), approvals = byId("agent-approvals");
const draft = byId<HTMLTextAreaElement>("agent-draft"), model = byId<HTMLSelectElement>("agent-model");
const notice = byId("agent-notice"), connection = byId("agent-connection");
const snapshots = new Map<string, AgentSession>();
const drafts = new Map<string, string>();
const itemNodes = new Map<string, { node: HTMLElement; text: string }>();
let roster: AgentSession[] = [], current = "", authenticated = false, polling = false, connected = false, submitting = false;

const pendingApprovals = new Set<string>();
const storedSession = "sterm-agent-selection";
try { current = sessionStorage.getItem(storedSession) ?? ""; } catch { /* 隐私模式仍使用内存状态。 */ }
const labels: Record<string, string> = { connecting: "连接中", idle: "空闲", running: "运行中", waiting: "等待审批", offline: "Agent 已断开", exited: "已退出" };
const login = new RemoteLogin(byId("login-panel"), () => { void authenticate(); });

function showNotice(text = ""): void { notice.textContent = text; notice.hidden = !text; }
async function request<T>(path: string, body?: unknown): Promise<T> {
  // 使用 POST 让浏览器附带 Origin；沿用原有远程服务的严格同源校验。
  const response = await fetch(path, { signal: AbortSignal.timeout(8000), method: "POST", headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), credentials: "same-origin", cache: "no-store" });
  if (response.status === 401) {
    authenticated = false; connected = false;
    byId("agent-layout").hidden = true;
    void login.check();
    throw new Error("访问授权已失效，请重新登录");
  }
  if (!response.ok) {
    if (response.status === 409) {
      const detail = await response.json().catch(() => ({})) as { message?: string };
      throw new Error(detail.message || "操作未被接受：请核对会话状态与电脑端分享权限");
    }
    if (response.status === 404) throw new Error("会话已结束或停止分享");
    throw new Error("连接失败，请稍后重试");
  }
  return response.json() as Promise<T>;
}
function active(): AgentSession | undefined { return snapshots.get(current); }
function writable(): boolean {
  const row = active();
  return Boolean(connected && row?.allowControl && !["offline","connecting","exited"].includes(row.state));
}
function syncControls(): void {
  const row = active(), write = writable();
  draft.disabled = !write || submitting;
  byId<HTMLButtonElement>("agent-send").disabled = !write || submitting || !draft.value.trim();
  byId<HTMLButtonElement>("agent-send").textContent = row?.state === "running" ? "追加" : "发送";
  byId<HTMLButtonElement>("agent-stop").disabled = !write || !row?.capabilities.stop || !row.turnId;
  byId("agent-policy-actions").hidden = !row?.capabilities.policy;
  for (const button of byId("agent-policy-actions").querySelectorAll<HTMLButtonElement>("button")) button.disabled = !write || row?.state !== "idle";
  model.disabled = !write || !row?.capabilities.models || row.state !== "idle";
  byId("agent-plan").hidden = !row?.capabilities.plan;
  byId<HTMLButtonElement>("agent-plan").disabled = !write || row?.state !== "idle";
  byId("agent-input-hint").textContent = !row ? "选择共享会话" : !connected ? "断线期间保留草稿，恢复后手动发送" : !row.allowControl ? "电脑端分享权限为只读" : row.state === "running" ? "运行期间可追加，由 Agent 安排执行" : "电脑端与网页均可操作";
  for (const button of approvals.querySelectorAll<HTMLButtonElement>("button")) button.disabled = !write || pendingApprovals.has(current + ":" + button.closest<HTMLElement>("[data-request]")?.dataset.request);
}
async function command(action: AgentAction): Promise<void> {
  const row = active();
  if (!row || !writable()) return;
  const identity = current, generation = row.generation;
  const id = Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, "0")).join("");
  try {
    const result = await request<{ id: string; status: string }>("/api/agents/command", {
      id, runtimeId: identity, generation, sessionId: row.sessionId, settingsRevision: row.settingsRevision, ...action,
    });
    if (identity === current) showNotice(result.status === "pending" ? "正在确认操作…" : "");
    // 只查询原请求结果，网络恢复后不重发输入。
    for (let attempt = 0; attempt < 60; attempt++) {
      await new Promise(resolve => setTimeout(resolve, 200));
      const receipt = await request<{ status: string; message?: string }>("/api/agents/" + identity + "/commands/" + id);
      if (receipt.status !== "pending" && receipt.status !== "dispatching") {
        if (identity === current) showNotice(receipt.status === "rejected" ? receipt.message || "Agent 拒绝了此操作" : "");
        if (receipt.status === "rejected") throw new Error(receipt.message || "Agent 拒绝了此操作");
        return;
      }
    }
    throw new Error("操作结果尚未确认，请核对会话后再操作；不会自动重发");
  } catch (error) { if (identity === current) showNotice(String(error instanceof Error ? error.message : error)); throw error; }
}
function renderRoster(rows: AgentSession[]): void {
  const key = rows.map(row => [row.runtimeId, row.title, row.state, row.allowControl].join("|")).join(";");
  if (list.dataset.key === key) return;
  list.dataset.key = key;
  list.replaceChildren();
  byId("agent-empty").hidden = rows.length > 0;
  for (const row of rows) {
    const button = document.createElement("button"); button.className = "agent-session"; button.dataset.runtime = row.runtimeId;
    button.setAttribute("aria-current", String(current === row.runtimeId));
    const name = document.createElement("strong"); name.textContent = row.title;
    const meta = document.createElement("small"); meta.textContent = (row.agent === "claude" ? "Claude" : "Codex") + " · " + (labels[row.state] ?? row.state);
    button.append(name, meta); button.onclick = () => select(row.runtimeId); list.append(button);
  }
}
function select(id: string): void {
  if (current) drafts.set(current, draft.value);
  current = id;
  try { sessionStorage.setItem(storedSession, id); } catch { /* 保留内存选择。 */ }
  let saved = "";
  try { saved = sessionStorage.getItem("sterm-agent-draft-" + id) ?? ""; } catch { /* 保留内存草稿。 */ }
  draft.value = drafts.get(id) ?? saved;
  itemNodes.clear(); messages.replaceChildren(); approvals.replaceChildren(); approvals.dataset.key = ""; showNotice();
  list.dataset.key = ""; renderRoster(roster);
  const row = snapshots.get(id);
  if (row) render(row);
  syncControls(); void poll();
}
function renderItem(item: AgentItem): HTMLElement {
  const card = document.createElement(item.kind === "tool" || item.kind === "thinking" ? "details" : "article");
  card.className = "agent-item agent-item-" + item.kind; card.dataset.item = item.id;
  const label = document.createElement(card instanceof HTMLDetailsElement ? "summary" : "div");
  label.className = "agent-item-label";
  label.textContent = item.kind === "user" ? "你" : item.kind === "assistant" ? item.title || "Agent" : (item.title || "工具") + " · " + (item.status === "running" || item.status === "inProgress" ? "执行中" : item.status === "error" || item.status === "failed" ? "失败" : item.status === "declined" ? "已拒绝" : item.status === "interrupted" ? "已中断" : "完成");
  const content = document.createElement("div"); content.className = "agent-content";
  if (item.kind === "tool" || item.kind === "user") {
    const pre = document.createElement("pre"); pre.textContent = item.text; content.append(pre);
  } else {
    content.innerHTML = DOMPurify.sanitize(marked.parse(item.text, { async: false }) as string, { FORBID_TAGS: ["img", "iframe", "style", "form"], FORBID_ATTR: ["style"] });
    for (const link of content.querySelectorAll("a")) { link.setAttribute("target", "_blank"); link.setAttribute("rel", "noopener noreferrer"); }
  }
  card.append(label, content);
  return card;
}
function render(row: AgentSession): void {
  if (row.runtimeId !== current) return;
  const atBottom = messages.scrollHeight - messages.scrollTop - messages.clientHeight < 70;
  byId("agent-title").textContent = row.title;
  byId("agent-path").textContent = row.cwd;
  byId("agent-state").textContent = labels[row.state] ?? row.state;
  byId("agent-state").dataset.state = row.state;
  byId("agent-policy").textContent = row.policy ? "当前权限：" + row.policy : "权限由原生 Agent 管理";
  const options = [...new Set([row.model, ...row.models])].filter(Boolean);
  if (model.dataset.key !== options.join("|")) {
    model.replaceChildren(...options.map(value => { const option = document.createElement("option"); option.value = value; option.textContent = value; return option; }));
    model.dataset.key = options.join("|");
  }
  model.value = row.model;
  const present = new Set(row.items.map(item => item.id));
  for (const [id, old] of itemNodes) if (!present.has(id)) { old.node.remove(); itemNodes.delete(id); }
  for (const item of row.items) {
    const signature = JSON.stringify(item), old = itemNodes.get(item.id);
    if (old?.text === signature) continue;
    const node = renderItem(item);
    if (old) {
      if (node instanceof HTMLDetailsElement && old.node instanceof HTMLDetailsElement) node.open = old.node.open;
      old.node.replaceWith(node);
    } else messages.append(node);
    itemNodes.set(item.id, { node, text: signature });
  }
  let position = messages.firstElementChild;
  for (const item of row.items) {
    const node = itemNodes.get(item.id)?.node;
    if (!node) continue;
    if (node !== position) messages.insertBefore(node, position);
    position = node.nextElementSibling;
  }
  const approvalKey = JSON.stringify(row.approvals);
  if (approvals.dataset.key !== approvalKey) {
    approvals.dataset.key = approvalKey; approvals.replaceChildren();
    for (const approval of row.approvals) {
      const card = document.createElement("section"); card.className = "agent-approval"; card.dataset.request = approval.id;
      const title = document.createElement("h3"); title.textContent = "需要审批 · " + approval.title;
      const detail = document.createElement("pre"); detail.textContent = approval.detail;
      const actions = document.createElement("div");
      for (const [allow, text] of [[false,"拒绝"],[true,"批准"]] as const) {
        const button = document.createElement("button"); button.textContent = text; button.dataset.decision = String(allow);
        button.onclick = () => {
          const key = current + ":" + approval.id;
          if (pendingApprovals.has(key)) return;
          pendingApprovals.add(key); syncControls();
          void command({ action: "respondApproval", requestId: approval.id, allow }).catch(() => {})
            .finally(() => { pendingApprovals.delete(key); syncControls(); });
        };
        actions.append(button);
      }
      card.append(title, detail, actions); approvals.append(card);
    }
  }
  if (row.error) showNotice(row.error);
  syncControls();
  if (atBottom) messages.scrollTop = messages.scrollHeight;
}
async function poll(): Promise<void> {
  if (polling || !authenticated) return;
  polling = true;
  try {
    const data = await request<AgentRoster>("/api/agents");
    roster = data.sessions;
    const alive = new Set(roster.map(row => row.runtimeId));
    for (const id of snapshots.keys()) if (!alive.has(id)) snapshots.delete(id);
    if (!current || !alive.has(current)) select(roster[0]?.runtimeId ?? "");
    if (current && !snapshots.has(current) && !draft.value) { try { draft.value = sessionStorage.getItem("sterm-agent-draft-" + current) ?? ""; } catch { /* 内存草稿仍可用。 */ } }
    if (!current) { messages.replaceChildren(); approvals.replaceChildren(); itemNodes.clear(); byId("agent-title").textContent = "选择一个会话"; byId("agent-path").textContent = ""; }
    renderRoster(roster);
    connected = true;
    const id = current;
    if (id) {
      const seq = snapshots.get(id)?.seq;
      const result = await request<{ unchanged?: boolean; snapshot?: AgentSession; changes?: AgentChange[] }>("/api/agents/" + id + (seq === undefined ? "" : "?seq=" + seq));
      let next = result.snapshot;
      if (result.changes) {
        const old = snapshots.get(id);
        next = old ? applyChanges(old, result.changes) ?? undefined : undefined;
        if (!next) {
          const fresh = await request<{ snapshot: AgentSession }>("/api/agents/" + id);
          next = fresh.snapshot;
        }
      }
      if (next) {
        if (next.protocolVersion !== 1) throw new Error("Agent 协议版本不兼容，请刷新页面或更新 sTerm");
        snapshots.set(id, next); if (id === current) render(next);
      }
    }
    connected = true; connection.textContent = "已连接"; connection.dataset.state = "connected";
    syncControls();
  } catch (error) {
    connected = false; connection.textContent = "连接已断开 · 正在重试"; connection.dataset.state = "offline";
    if (authenticated) showNotice(error instanceof Error ? error.message : String(error));
    syncControls();
  } finally { polling = false; }
}
async function authenticate(): Promise<void> {
  try {
    const response = await fetch("/api/auth", { credentials: "same-origin", cache: "no-store" });
    const status = await response.json() as { authenticated: boolean };
    if (!response.ok || !status.authenticated) { void login.check(); return; }
    login.hide(); authenticated = true;
    byId("agent-layout").hidden = false; await poll();
  } catch { void login.check(); }
}
draft.oninput = () => { if (current) {
    drafts.set(current, draft.value);
    try { sessionStorage.setItem("sterm-agent-draft-" + current, draft.value); } catch { /* 配额不足时保留内存草稿。 */ }
  }
  syncControls(); };
byId<HTMLFormElement>("agent-composer").onsubmit = event => {
  event.preventDefault();
  if (!draft.value.trim() || submitting || !writable()) return;
  const text = draft.value, id = current;
  submitting = true; syncControls();
  void command({ action: "sendText", text }).then(() => {
    if (drafts.get(id) === text) drafts.delete(id);
    try { if (sessionStorage.getItem("sterm-agent-draft-" + id) === text) sessionStorage.removeItem("sterm-agent-draft-" + id); } catch { /* 保留其他会话。 */ }
    if (current === id && draft.value === text) draft.value = "";
  }).catch(() => {}).finally(() => { submitting = false; syncControls(); });
};
draft.onkeydown = event => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.isComposing) { event.preventDefault(); byId<HTMLFormElement>("agent-composer").requestSubmit(); } };
byId("agent-stop").onclick = () => { const row = active(); if (row?.turnId) void command({ action: "stopTurn", turnId: row.turnId }).catch(() => {}); };
byId("agent-plan").onclick = () => { void command({ action: "enterPlan" }).catch(() => {}); };
model.onchange = () => { void command({ action: "setModel", model: model.value }).catch(() => { const row = active(); if (row) model.value = row.model; }); };
for (const button of byId("agent-policy-actions").querySelectorAll<HTMLButtonElement>("button")) {
  button.onclick = () => { void command({ action: "setPolicy", sandbox: button.dataset.sandbox!, approvalPolicy: "on-request" }).catch(() => {}); };
}
window.setInterval(() => void poll(), 250);
const pair = new URLSearchParams(location.hash.slice(1)).get("pair");
if (pair) {
  void pairRemoteBrowser(pair, false).then(() => { history.replaceState(null, "", location.pathname); void authenticate(); }).catch(error => { void login.check(String(error)); });
} else void authenticate();
