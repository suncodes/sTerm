import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { OverlaySnapshot } from "./types";

const list = document.getElementById("session-list")!;
const collapseButton = document.getElementById("collapse-overlay") as HTMLButtonElement;
const close = document.getElementById("close-overlay") as HTMLButtonElement;
const errorEl = document.getElementById("overlay-error")!;
const appWindow = getCurrentWindow();
const labels = { running: "运行中", idle: "空闲", exited: "已退出" };
const agents = { claude: "Claude", codex: "Codex", pi: "Pi" };
let revision = -1;
let collapsed = false;
let expandPending = false;

function report(error: unknown): void {
  errorEl.textContent = "操作失败：" + String(error);
  errorEl.hidden = false;
}

function expand(): void {
  if (!collapsed || expandPending) return;
  expandPending = true;
  errorEl.hidden = true;
  void invoke<OverlaySnapshot>("overlay_expand")
    .then(render)
    .catch(report)
    .finally(() => { expandPending = false; });
}

function render(snapshot: OverlaySnapshot): void {
  if (snapshot.revision < revision) return;
  revision = snapshot.revision;
  collapsed = snapshot.collapsed && snapshot.dock !== null;
  document.body.classList.toggle("collapsed", collapsed);
  document.body.classList.remove("dock-left", "dock-right", "dock-top", "dock-bottom");
  if (collapsed && snapshot.dock) document.body.classList.add("dock-" + snapshot.dock);
  collapseButton.hidden = collapsed || snapshot.dock === null || snapshot.sessions.length === 0;
  document.documentElement.style.setProperty("--panel-opacity", String(snapshot.background_opacity / 100));
  document.getElementById("session-count")!.textContent = String(snapshot.sessions.length);
  const scroll = list.scrollTop;
  const fragment = document.createDocumentFragment();
  if (snapshot.sessions.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "暂无 Agent 会话";
    fragment.appendChild(empty);
  }
  for (const session of snapshot.sessions) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "session" + (session.active ? " active" : "") + (collapsed ? " collapsed-session" : "");
    row.dataset.sessionId = String(session.id);
    const status = labels[session.status];
    row.title = collapsed
      ? `${session.title} · ${agents[session.agent]} · ${status}（点击展开悬浮窗）`
      : `${session.title} · ${agents[session.agent]} · ${status}${session.active ? " · 当前标签" : ""}（点击返回会话）`;
    row.setAttribute("aria-label", row.title);
    if (session.active && !collapsed) row.setAttribute("aria-current", "true");
    const dot = document.createElement("span");
    dot.className = "dot " + session.status;
    dot.setAttribute("aria-hidden", "true");
    const copy = document.createElement("span");
    copy.className = "copy";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = session.title;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = agents[session.agent];
    copy.append(name, meta);
    const badge = document.createElement("span");
    badge.className = "status " + session.status;
    badge.textContent = status;
    row.append(dot, copy, badge);
    row.onclick = () => {
      errorEl.hidden = true;
      if (collapsed) {
        expand();
      } else {
        void invoke("overlay_activate", { id: session.id }).catch(report);
      }
    };
    fragment.appendChild(row);
  }
  list.replaceChildren(fragment);
  list.scrollTop = scroll;
}

collapseButton.onclick = () => {
  collapseButton.disabled = true;
  errorEl.hidden = true;
  void invoke<OverlaySnapshot>("overlay_collapse")
    .then(render)
    .catch(report)
    .finally(() => { collapseButton.disabled = false; });
};

close.onclick = async () => {
  close.disabled = true;
  errorEl.hidden = true;
  try {
    await invoke("overlay_set_enabled", { enabled: false });
  } catch (error) {
    report(error);
  } finally {
    close.disabled = false;
  }
};

let dragArmed = false;
let movedDuringDrag = false;
let dragTimer: number | undefined;
let armTimer: number | undefined;

function finishDragging(): void {
  window.clearTimeout(dragTimer);
  window.clearTimeout(armTimer);
  dragTimer = undefined;
  armTimer = undefined;
  const shouldSave = dragArmed && movedDuringDrag;
  dragArmed = false;
  movedDuringDrag = false;
  if (shouldSave) void invoke<OverlaySnapshot>("overlay_save_position").then(render).catch(report);
}

function scheduleFinish(delay: number): void {
  window.clearTimeout(dragTimer);
  dragTimer = window.setTimeout(finishDragging, delay);
}

document.getElementById("drag-handle")!.addEventListener("mousedown", (e) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  e.preventDefault();
  dragArmed = true;
  movedDuringDrag = false;
  window.clearTimeout(armTimer);
  armTimer = window.setTimeout(() => {
    if (!movedDuringDrag) dragArmed = false;
  }, 1000);
  void appWindow.startDragging().catch((error) => {
    dragArmed = false;
    report(error);
  });
});
document.addEventListener("mouseup", () => {
  // Windows 的系统拖动结束后，最后一个 moved 事件可能稍后才到达。
  if (dragArmed) scheduleFinish(500);
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

async function main(): Promise<void> {
  await listen<OverlaySnapshot>("overlay-changed", (e) => render(e.payload));
  await appWindow.onMoved(() => {
    if (!dragArmed) return;
    movedDuringDrag = true;
    window.clearTimeout(armTimer);
    scheduleFinish(450);
  });
  render(await invoke<OverlaySnapshot>("overlay_ready"));
}

void main().catch(report);
