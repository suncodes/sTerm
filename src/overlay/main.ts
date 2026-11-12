import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { OverlaySnapshot } from "./types";

const list = document.getElementById("session-list")!;
const close = document.getElementById("close-overlay") as HTMLButtonElement;
const errorEl = document.getElementById("overlay-error")!;
const appWindow = getCurrentWindow();
const labels = { running: "运行中", idle: "空闲", exited: "已退出" };
const agents = { claude: "Claude", codex: "Codex", pi: "Pi" };
let revision = -1;

function report(error: unknown): void {
  errorEl.textContent = "操作失败：" + String(error);
  errorEl.hidden = false;
}

function render(snapshot: OverlaySnapshot): void {
  if (snapshot.revision < revision) return;
  revision = snapshot.revision;
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
    row.className = "session" + (session.active ? " active" : "");
    row.dataset.sessionId = String(session.id);
    const status = labels[session.status];
    row.title = `${session.title} · ${agents[session.agent]} · ${status}${session.active ? " · 当前标签" : ""}（点击返回会话）`;
    row.setAttribute("aria-label", row.title);
    if (session.active) row.setAttribute("aria-current", "true");
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
      void invoke("overlay_activate", { id: session.id }).catch(report);
    };
    fragment.appendChild(row);
  }
  list.replaceChildren(fragment);
  list.scrollTop = scroll;
}

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

document.getElementById("drag-handle")!.addEventListener("mousedown", (e) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  e.preventDefault();
  void appWindow.startDragging().catch(report);
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

async function main(): Promise<void> {
  await listen<OverlaySnapshot>("overlay-changed", (e) => render(e.payload));
  let saveTimer: number | undefined;
  await appWindow.onMoved(() => {
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      void invoke("overlay_save_position").catch(report);
    }, 350);
  });
  render(await invoke<OverlaySnapshot>("overlay_ready"));
}

void main().catch(report);
