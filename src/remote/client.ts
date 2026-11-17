import "@xterm/xterm/css/xterm.css";
import "./mobile.css";
import { Terminal } from "@xterm/xterm";
import { terminalKey, terminalInput, type SharedSession } from "./types";
import { TerminalMirror } from "./mirror";
import { AGENT_STATUS_LABELS } from "../terminal/agentstate";

const element = <ElementType extends HTMLElement>(id: string): ElementType => document.getElementById(id) as ElementType;
const app = element("remote-app");
const connection = element("connection");
const draft = element<HTMLTextAreaElement>("draft");
const scroll = element("screen-scroll");
const backdrop = element("sheet-backdrop");
const terminal = new Terminal({
  cols: 80, rows: 24, fontSize: 13, fontFamily: '"Cascadia Mono", Consolas, "Microsoft YaHei UI", monospace',
  scrollback: 1000, disableStdin: true, cursorBlink: false,
  theme: {
    background: "#000000", foreground: "#CCCCCC", cursor: "#FFFFFF", cursorAccent: "#0C0C0C",
    selectionBackground: "#FFFFFF", selectionForeground: "#000000",
    black: "#0C0C0C", red: "#C50F1F", green: "#13A10E", yellow: "#C19C00",
    blue: "#0037DA", magenta: "#881798", cyan: "#3A96DD", white: "#CCCCCC",
    brightBlack: "#767676", brightRed: "#E74856", brightGreen: "#16C60C", brightYellow: "#F9F1A5",
    brightBlue: "#3B78FF", brightMagenta: "#B4009E", brightCyan: "#61D6D6", brightWhite: "#F2F2F2",
  },
});
terminal.open(element("screen"));
const mirror = new TerminalMirror(terminal, updateSize);
const drafts = new Map<string, string>();

let socket: WebSocket | undefined;
let retryTimer: number | undefined;
let heartbeat: number | undefined;
let resizeTimer: number | undefined;
let reconnectDelay = 1000;
let shared: SharedSession[] = [];
let current: SharedSession | undefined;
let attachedKey = "";
let clientId = "";
let screenReady = false;
let lastSeq = 0;
let awaitingSnapshot = true;
let ctrlPressed = false;
let wasControlling = false;
let revoked = false;
let sessionsKey = "";
let sheetFocus: HTMLElement | null = null;

const sessionKey = (session: SharedSession): string => session.id + "/" + session.generation;
const connected = (): boolean => socket?.readyState === WebSocket.OPEN;
function showNotice(text = ""): void {
  element("notice").textContent = text;
  element("notice").hidden = !text;
}
function connectionState(text: string, state: string): void {
  connection.textContent = text;
  connection.dataset.state = state;
}
function send(message: unknown): boolean {
  if (!socket || socket.readyState !== WebSocket.OPEN || socket.bufferedAmount > 32 * 1024) return false;
  socket.send(JSON.stringify(message));
  return true;
}
function ownsControl(): boolean {
  return screenReady && connected() && Boolean(current?.allowControl && current.controller === clientId && current.epoch);
}
function setCtrl(pressed: boolean): void {
  ctrlPressed = pressed;
  element("ctrl-key").setAttribute("aria-pressed", String(pressed));
}
function updateStatusDot(dot: HTMLElement, status: SharedSession["agentStatus"]): string {
  const label = status ? AGENT_STATUS_LABELS[status] ?? "" : "";
  dot.dataset.status = status ?? "none";
  dot.hidden = !label;
  dot.title = label;
  dot.setAttribute("aria-hidden", "true");
  return label;
}
function syncControls(): void {
  const controls = ownsControl();
  const available = screenReady && connected() && Boolean(current);
  app.dataset.controlling = String(controls);
  element("claim").hidden = controls || !current?.allowControl;
  element<HTMLButtonElement>("claim").disabled = !available || Boolean(current?.controller);
  element("release").hidden = !controls;
  element<HTMLButtonElement>("release").disabled = !controls;
  element<HTMLButtonElement>("resize").disabled = !controls;
  element<HTMLInputElement>("fit-width").disabled = !controls;
  element("input-tools").hidden = !controls;
  element("readonly-hint").hidden = controls;
  element("control-state").textContent = controls ? "控制中" : current?.controller ? "其他设备控制中" : "只读";
  element("readonly-description").textContent = !connected() ? "连接恢复后，需重新接管才能输入"
    : !current ? "在电脑端开启会话分享后，即可查看终端"
    : !screenReady ? "正在同步终端画面…"
    : current.controller ? "其他设备正在控制，请等待归还"
    : current.allowControl ? "点击右上方「接管」后即可输入" : "电脑端尚未允许此会话输入";
  element<HTMLButtonElement>("send").disabled = !controls || !draft.value;
  draft.disabled = !controls;
  element<HTMLInputElement>("append-enter").disabled = !controls;
  document.querySelectorAll<HTMLButtonElement>("#keys button, #extra-keys button").forEach(button => { button.disabled = !controls; });
  element("empty-screen").hidden = Boolean(current);
  element("current-title").textContent = current?.title ?? "等待分享会话";
  element("current-title").title = current?.title ?? "";
  const statusLabel = updateStatusDot(element("current-dot"), current?.agentStatus);
  element("current-status").textContent = statusLabel;
  element("current-status").hidden = !statusLabel;
  element("session-path").textContent = current?.cwd ?? "";
  element("session-path").title = current?.cwd ?? "";
  if (!controls) setCtrl(false);
  if (controls && !wasControlling) scheduleFit();
  wasControlling = controls;
}
function rememberDraft(): void {
  if (current) drafts.set(sessionKey(current), draft.value);
}
function renderSessions(): void {
  const key = JSON.stringify([shared, current && sessionKey(current)]);
  if (key === sessionsKey) return;
  sessionsKey = key;
  const focused = document.activeElement as HTMLElement | null;
  const focusedKey = focused?.dataset.sessionKey;
  const focusInTabs = Boolean(focused && element("session-tabs").contains(focused));
  const tabList = element("session-tabs");
  const list = element("session-list");
  tabList.replaceChildren();
  list.replaceChildren();
  if (!shared.length) {
    const empty = document.createElement("div");
    empty.className = "sheet-empty";
    empty.textContent = "暂无分享会话";
    list.append(empty);
  }
  for (const session of shared) {
    const selected = Boolean(current && sessionKey(session) === sessionKey(current));
    const tab = document.createElement("button");
    tab.className = "client-tab";
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
    tab.dataset.sessionKey = sessionKey(session);
    const dot = document.createElement("span");
    dot.className = "status-dot";
    const statusLabel = updateStatusDot(dot, session.agentStatus);
    tab.title = [session.title, session.agent, statusLabel].filter(Boolean).join(" · ");
    const label = document.createElement("span");
    label.className = "tab-label"; label.textContent = session.title;
    const badge = document.createElement("span");
    badge.className = "agent-status";
    badge.textContent = statusLabel;
    badge.hidden = !statusLabel;
    tab.append(dot, label, badge);
    tab.onclick = () => attach(session);
    tabList.append(tab);
    const row = document.createElement("button");
    row.className = "sheet-session";
    row.dataset.sessionKey = sessionKey(session);
    row.setAttribute("aria-current", String(selected));
    const description = document.createElement("span");
    const title = document.createElement("strong");
    title.textContent = session.title;
    const detail = document.createElement("small");
    detail.textContent = [session.agent, statusLabel, session.controller ? "控制中" : session.allowControl ? "可接管" : "只读", session.cwd].filter(Boolean).join(" · ");
    row.title = session.title + " · " + detail.textContent;
    description.append(title, detail);
    const mark = document.createElement("span");
    mark.className = "selected-mark"; mark.textContent = selected ? "✓" : "";
    row.append(dot.cloneNode(true), description, mark);
    row.onclick = () => { closeSheet(); if (!selected) attach(session); };
    list.append(row);
  }
  if (focusedKey && !backdrop.hidden && backdrop.dataset.sheet === "sessions") {
    [...list.querySelectorAll<HTMLButtonElement>("button")].find(button => button.dataset.sessionKey === focusedKey)?.focus();
  } else if (focusedKey && focusInTabs) {
    [...tabList.querySelectorAll<HTMLButtonElement>("button")].find(button => button.dataset.sessionKey === focusedKey)?.focus();
  }
}
function updateSize(cols: number, rows: number): void {
  if (terminal.cols !== cols || terminal.rows !== rows) terminal.resize(cols, rows);
  element("dimensions").textContent = cols + " × " + rows;
  const rect = terminal.element?.querySelector(".xterm-screen")?.getBoundingClientRect();
  const font = terminal.options.fontSize!;
  element("screen").style.width = Math.ceil((rect?.width || cols * font * .63) + 24) + "px";
  element("screen").style.minHeight = Math.ceil((rect?.height || rows * font * 1.2) + 16) + "px";
}
function attach(session: SharedSession): void {
  rememberDraft();
  current = session;
  draft.value = drafts.get(sessionKey(session)) ?? "";
  screenReady = false;
  awaitingSnapshot = true;
  mirror.invalidate(true);
  lastSeq = 0;
  showNotice();
  syncControls();
  renderSessions();
  connectionState("正在同步…", "connecting");
  attachedKey = send({ type: "attach", id: session.id, generation: session.generation }) ? sessionKey(session) : "";
}
function roster(list: SharedSession[]): void {
  shared = list;
  const selected = current && shared.find(session => sessionKey(session) === sessionKey(current!));
  for (const key of drafts.keys()) if (!shared.some(session => sessionKey(session) === key)) drafts.delete(key);
  if (!shared.length) {
    current = undefined; draft.value = "";
    attachedKey = "";
    screenReady = false; awaitingSnapshot = true;
    mirror.invalidate(true);
    connectionState("已连接", "connected");
    syncControls(); renderSessions();
  } else if (selected && attachedKey === sessionKey(selected)) {
    current = selected;
    syncControls(); renderSessions();
  } else {
    attach(selected ?? shared[0]);
  }
}
function disconnected(): void {
  rememberDraft();
  screenReady = false; awaitingSnapshot = true; attachedKey = "";
  mirror.invalidate();
  if (current) current = { ...current, controller: null, epoch: null };
  element("offline-banner").hidden = false;
  element("offline-message").textContent = revoked ? "设备授权已撤销，请重新扫码配对" : "连接已断开 · 终端画面与草稿已保留";
  connectionState(revoked ? "需要配对" : "等待重连", "offline");
  syncControls(); renderSessions();
}
function connect(): void {
  window.clearTimeout(retryTimer);
  window.clearInterval(heartbeat);
  const previous = socket;
  socket = undefined;
  previous?.close();
  disconnected();
  connectionState("正在连接…", "connecting");
  element("offline-banner").hidden = !current;
  const next = new WebSocket((location.protocol === "https:" ? "wss:" : "ws:") + "//" + location.host + "/api/ws");
  socket = next;
  next.onmessage = event => {
    if (socket !== next) return;
    const message = JSON.parse(String(event.data));
    if (message.type === "hello") {
      clientId = message.clientId;
      revoked = false; reconnectDelay = 1000;
      element("offline-banner").hidden = true;
      showNotice();
      roster(message.sessions);
      heartbeat = window.setInterval(() => send({ type: "ping" }), 15000);
    } else if (message.type === "sessions") {
      roster(message.sessions);
    } else if ((message.type === "snapshot" || message.type === "output") && current && current.id === message.id && current.generation === message.generation) {
      if (message.type === "snapshot") {
        awaitingSnapshot = false;
        lastSeq = message.seq;
      } else {
        if (awaitingSnapshot || message.seq <= lastSeq) return;
        if (message.seq !== lastSeq + 1) { attach(current); return; }
        lastSeq = message.seq;
      }
      mirror.write({ ...message, snapshot: message.type === "snapshot" }, () => {
        if (socket !== next) return;
        if (message.type === "snapshot") {
          screenReady = true;
          connectionState("已连接", "connected");
          syncControls();
        }
        send({ type: "ack", seq: message.seq });
        updateJump();
      });
    } else if (message.type === "error") {
      showNotice(message.message);
    } else if (message.type === "revoked") {
      revoked = true;
      showNotice("电脑端已撤销设备授权，请重新扫码。草稿保留在当前页面。");
      next.close();
    } else if (message.type === "stopped") {
      showNotice("电脑端已关闭远程访问，终端继续在电脑上运行。");
      next.close();
    }
  };
  next.onclose = () => {
    if (socket !== next) return;
    window.clearInterval(heartbeat);
    disconnected();
    if (!revoked) {
      retryTimer = window.setTimeout(connect, reconnectDelay);
      reconnectDelay = Math.min(15000, reconnectDelay * 2);
    }
  };
  next.onerror = () => {
    if (socket === next) showNotice("连接失败，正在重试。若设备授权已过期，请在电脑端重新扫码。");
  };
}
function input(data: string): boolean {
  if (!data || !ownsControl() || !current) return false;
  if (new TextEncoder().encode(data).length > 8192) { showNotice("每次输入不能超过 8 KB，请分段发送。"); return false; }
  if (!send({ type: "input", id: current.id, generation: current.generation, epoch: current.epoch, data })) {
    showNotice("发送失败，请确认连接后重试；草稿未自动重发。");
    return false;
  }
  showNotice();
  return true;
}
function submit(): void {
  if (!draft.value) return;
  if (input(terminalInput(draft.value, element<HTMLInputElement>("append-enter").checked, terminal.modes.bracketedPasteMode))) {
    draft.value = "";
    rememberDraft();
    syncControls();
  }
}
function openSheet(name: "sessions" | "keys" | "display"): void {
  if (backdrop.hidden) sheetFocus = document.activeElement as HTMLElement | null;
  backdrop.dataset.sheet = name;
  element("sheet-title").textContent = name === "sessions" ? "所有会话 · " + shared.length : name === "keys" ? "更多快捷键" : "显示与连接";
  element("session-list").hidden = name !== "sessions";
  element("extra-keys").hidden = name !== "keys";
  element("display-options").hidden = name !== "display";
  backdrop.hidden = false;
  element("all-sessions").setAttribute("aria-expanded", String(name === "sessions"));
  for (const child of app.children) if (child instanceof HTMLElement && child !== backdrop) child.inert = true;
  const selected = name === "sessions" ? element("session-list").querySelector<HTMLButtonElement>('[aria-current="true"]') : null;
  (selected ?? element("close-sheet")).focus();
}
function closeSheet(): void {
  backdrop.hidden = true;
  element("all-sessions").setAttribute("aria-expanded", "false");
  for (const child of app.children) if (child instanceof HTMLElement) child.inert = false;
  if (sheetFocus?.getClientRects().length && !sheetFocus.closest("[hidden]")) sheetFocus.focus();
  else scroll.focus();
}
function fitTerminal(): void {
  if (!current || !ownsControl()) return;
  const rect = terminal.element?.querySelector(".xterm-screen")?.getBoundingClientRect();
  const cellWidth = rect?.width ? rect.width / terminal.cols : terminal.options.fontSize! * .63;
  const cellHeight = rect?.height ? rect.height / terminal.rows : terminal.options.fontSize! * 1.2;
  const cols = Math.max(20, Math.min(400, Math.floor((scroll.clientWidth - 24) / cellWidth)));
  const rows = Math.max(5, Math.min(200, Math.floor((scroll.clientHeight - 16) / cellHeight)));
  if (cols === terminal.cols && rows === terminal.rows) return;
  send({ type: "resize", id: current.id, generation: current.generation, epoch: current.epoch, cols, rows });
}
function scheduleFit(): void {
  window.clearTimeout(resizeTimer);
  if (element<HTMLInputElement>("fit-width").checked && ownsControl()) resizeTimer = window.setTimeout(fitTerminal, 180);
}
function updateJump(): void {
  const buffer = terminal.buffer.active;
  element("terminal-jump").hidden = !current || buffer.viewportY >= buffer.baseY;
}

element("all-sessions").onclick = () => openSheet("sessions");
element("display-menu").onclick = () => openSheet("display");
element("more-keys").onclick = () => openSheet("keys");
element("close-sheet").onclick = closeSheet;
element("menu-sessions").onclick = () => openSheet("sessions");
element("menu-reconnect").onclick = () => { closeSheet(); connect(); };
element("reconnect").onclick = connect;
backdrop.onclick = event => { if (event.target === backdrop) closeSheet(); };
backdrop.onkeydown = event => {
  if (event.key === "Escape") { event.preventDefault(); closeSheet(); }
  if (event.key !== "Tab") return;
  const focusable = [...backdrop.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")].filter(node => node.getClientRects().length > 0);
  const first = focusable[0], last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
};
element("session-tabs").onkeydown = event => {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  const tabs = [...element("session-tabs").querySelectorAll<HTMLButtonElement>("button")];
  if (!tabs.length) return;
  const index = tabs.indexOf(document.activeElement as HTMLButtonElement);
  const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
  event.preventDefault();
  const session = shared[next];
  attach(session);
  element("session-tabs").querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
};
element("claim").onclick = () => { if (current && screenReady) send({ type: "claim", id: current.id, generation: current.generation }); };
element("release").onclick = () => { if (current) send({ type: "release", id: current.id, generation: current.generation }); };
element("send").onclick = submit;
draft.oninput = () => { rememberDraft(); element<HTMLButtonElement>("send").disabled = !ownsControl() || !draft.value; };
draft.onkeydown = event => {
  if (event.key === "Enter" && event.ctrlKey && !event.isComposing && event.keyCode !== 229) { event.preventDefault(); submit(); }
};
element("ctrl-key").onclick = () => setCtrl(!ctrlPressed);
document.querySelectorAll<HTMLButtonElement>("[data-key]").forEach(button => {
  button.onclick = () => {
    const sent = input(terminalKey(button.dataset.key!, terminal.modes.applicationCursorKeysMode, ctrlPressed));
    if (sent) {
      setCtrl(false);
      if (button.closest("#extra-keys")) closeSheet();
    }
  };
});
for (const [id, delta] of [["font-smaller", -1], ["font-larger", 1]] as const) {
  element(id).onclick = () => {
    terminal.options.fontSize = Math.max(8, Math.min(24, terminal.options.fontSize! + delta));
    element("font-value").textContent = terminal.options.fontSize + " px";
    element<HTMLButtonElement>("font-smaller").disabled = terminal.options.fontSize === 8;
    element<HTMLButtonElement>("font-larger").disabled = terminal.options.fontSize === 24;
    requestAnimationFrame(() => { updateSize(terminal.cols, terminal.rows); scheduleFit(); });
  };
}
element("resize").onclick = fitTerminal;
element<HTMLInputElement>("fit-width").onchange = scheduleFit;
element("scroll-bottom").onclick = () => { terminal.scrollToBottom(); scroll.scrollTop = scroll.scrollHeight; updateJump(); };
terminal.onScroll(updateJump);
new ResizeObserver(scheduleFit).observe(scroll);
function viewport(): void {
  document.documentElement.style.setProperty("--viewport-height", (window.visualViewport?.height ?? window.innerHeight) + "px");
}
viewport();
window.visualViewport?.addEventListener("resize", viewport);
window.addEventListener("resize", viewport);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && !connected() && !revoked) connect();
});

async function start(): Promise<void> {
  const token = new URLSearchParams(location.hash.slice(1)).get("pair");
  history.replaceState(null, "", location.pathname);
  if (token) {
    const response = await fetch("/api/pair", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ token }), credentials: "same-origin" });
    if (!response.ok) throw new Error("配对二维码已失效或已使用，请在电脑端重新生成。");
  }
  connect();
}
function pairingFailed(error: unknown): void {
  revoked = true;
  disconnected();
  connectionState("配对失败", "offline");
  showNotice(error instanceof Error ? error.message : String(error));
}
window.addEventListener("hashchange", () => {
  if (new URLSearchParams(location.hash.slice(1)).has("pair")) void start().catch(pairingFailed);
});
updateSize(80, 24);
syncControls();
void start().catch(pairingFailed);
