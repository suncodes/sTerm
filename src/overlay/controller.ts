import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { collectAgentSessions, type OverlaySnapshot } from "./types";
import { OverlayKeepAlive } from "./keepalive";

/** 主窗口只发布摘要，小窗不读取 PTY，也不维护第二套状态机。 */
export class AgentOverlay {
  private last = "";
  private readonly keepAlive = new OverlayKeepAlive();
  private revision = -1;
  private pending: Promise<unknown> = Promise.resolve();

  constructor(private readonly activate: (id: number) => void) {}

  async init(): Promise<void> {
    await listen<number>("overlay-activate", (e) => this.activate(e.payload));
    const apply = (snapshot: OverlaySnapshot): void => {
      if (snapshot.revision < this.revision) return;
      this.revision = snapshot.revision;
      this.keepAlive.setEnabled(snapshot.enabled);
    };
    await listen<OverlaySnapshot>("overlay-changed", (e) => apply(e.payload));
    apply(await invoke<OverlaySnapshot>("overlay_init"));
    window.addEventListener("beforeunload", () => this.keepAlive.setEnabled(false), { once: true });
  }

  sync(...args: Parameters<typeof collectAgentSessions>): void {
    const sessions = collectAgentSessions(...args);
    const signature = JSON.stringify(sessions);
    if (signature === this.last) return;
    this.last = signature;
    // 按产生顺序提交，防止快速切换标签时旧摘要覆盖新摘要。
    this.pending = this.pending
      .then(() => invoke("overlay_publish", { sessions }))
      .catch((error) => {
        this.last = "";
        console.error("悬浮窗状态同步失败", error);
      });
  }
}
