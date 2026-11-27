import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { invoke } from "@tauri-apps/api/core";
import type { TabManager } from "../terminal/tabmanager";
import type { RemoteController } from "../remote/controller";
import type { AgentRoster, AgentSession } from "./types";
import { showToast } from "../ui/toast";

export function setupAgentLauncher(tabs: TabManager, remote: RemoteController): void {
  const button = document.getElementById("btn-agent-mode");
  button?.addEventListener("click", () => { void open(); });
  async function open(): Promise<void> {
    const existing = document.getElementById("agent-desktop-dialog");
    if (existing instanceof HTMLDialogElement) { existing.showModal(); return; }
    const dialog = document.createElement("dialog");
    dialog.id = "agent-desktop-dialog";
    dialog.className = "agent-desktop-dialog";
    const heading = document.createElement("h2"); heading.textContent = "Agent 会话";
    const close = document.createElement("button"); close.textContent = "关闭"; close.className = "agent-dialog-close";
    close.onclick = () => dialog.close();
    const explanation = document.createElement("p"); explanation.textContent = "原生终端与网页共享会话，两侧均可操作。";
    const controls = document.createElement("div"); controls.className = "agent-launch-controls";
    const directory = document.createElement("input"); directory.value = tabs.active?.cwd ?? ""; directory.placeholder = "工作目录，例如 D:\\workspace\\project"; directory.setAttribute("aria-label", "Agent 工作目录");
    const resume = document.createElement("input"); resume.placeholder = "历史会话 ID（可选；留空新建）"; resume.setAttribute("aria-label", "恢复历史会话 ID");
    controls.append(directory, resume);
    for (const agent of ["claude", "codex"] as const) {
      const start = document.createElement("button"); start.textContent = "启动 " + (agent === "claude" ? "Claude" : "Codex");
      start.onclick = async () => {
        if (!directory.value.trim()) { directory.focus(); return; }
        start.disabled = true;
        try {
          const session = await tabs.newTab(agent === "claude" ? "Claude Agent" : "Codex Agent", directory.value.trim());
          await session.startAgent(agent, directory.value.trim(), resume.value.trim() || undefined);
          await refresh();
        } catch (error) { showToast("启动失败：" + String(error)); }
        finally { start.disabled = false; }
      };
      controls.append(start);
    }
    const list = document.createElement("div"); list.className = "agent-desktop-list";
    const link = document.createElement("button"); link.textContent = "开启本机网页访问";
    link.onclick = async () => {
      try {
        if (!remote.status) await remote.start(0, false);
        const origin = remote.status?.origins.find(value => value.startsWith("https://")) ?? remote.status?.origins[0];
        if (origin) {
          const pair = await invoke<{ url: string; token?: string }>("remote_pair", { origin });
          const url = pair.url.replace("/#pair=", "/agent.html#pair=");
          await writeText(url);
          showToast("Agent 网页配对链接已复制");
        }
      } catch (error) { showToast(String(error)); }
    };
    dialog.append(heading, close, explanation, controls, list, link);
    document.body.append(dialog); dialog.showModal();
    let timer: number;
    dialog.onclose = () => { window.clearInterval(timer); dialog.remove(); };
    let loading = false;
    async function share(row: AgentSession, mode: string): Promise<void> {
      try {
        await invoke("agent_runtime_share", { runtimeId: row.runtimeId, generation: row.generation, shared: mode !== "off", allowControl: mode === "control" });
        await refresh();
      } catch (error) { showToast(String(error)); }
    }
    async function refresh(): Promise<void> {
      if (loading || !dialog.open) return;
      loading = true;
      try {
        const { sessions } = await invoke<AgentRoster>("agent_runtime_list");
        const focused = document.activeElement instanceof HTMLElement ? document.activeElement.dataset.runtime : undefined;
        list.replaceChildren();
        if (!sessions.length) { const empty = document.createElement("p"); empty.textContent = "暂无已接入会话，请从这里启动 Agent。"; list.append(empty); }
        for (const row of sessions) {
          const card = document.createElement("div"); card.className = "agent-desktop-row";
          const label = document.createElement("span"); label.textContent = row.title + " · " + ({idle:"空闲",running:"运行中",waiting:"等待审批",connecting:"连接中",offline:"已断开"}[row.state] ?? row.state);
          const path = document.createElement("small"); path.textContent = row.cwd; label.append(path);
          const permission = document.createElement("select"); permission.dataset.runtime = row.runtimeId;
          permission.setAttribute("aria-label", row.title + " 分享权限");
          for (const [value, text] of [["off","不分享"],["readonly","分享 · 只读"],["control","分享 · 允许操作"]]) {
            const option = document.createElement("option"); option.value = value; option.textContent = text; permission.append(option);
          }
          permission.value = row.shared ? row.allowControl ? "control" : "readonly" : "off";
          permission.onchange = () => void share(row, permission.value);
          card.append(label, permission); list.append(card);
        }
        if (focused) list.querySelector<HTMLElement>('[data-runtime="' + focused + '"]')?.focus();
        link.textContent = remote.status ? "复制 Agent 网页配对链接" : "开启本机网页访问并复制链接";
      } catch (error) { showToast("读取 Agent 会话失败：" + String(error)); }
      finally { loading = false; }
    }
    await refresh();
    timer = window.setInterval(() => { if (!list.contains(document.activeElement)) void refresh(); }, 1500);
  }
}
