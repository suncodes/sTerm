/** 仅在悬浮窗开启时持有共享 Web Lock，避免主 WebView 在后台被冻结。 */
export class OverlayKeepAlive {
  private release: (() => void) | undefined;

  setEnabled(enabled: boolean): void {
    if (!enabled) {
      this.release?.();
      this.release = undefined;
      return;
    }
    if (this.release || !navigator.locks) return;
    // 多个 sTerm 实例可同时持有；关闭前已创建的 Promise 也能正常释放。
    const held = new Promise<void>((resolve) => { this.release = resolve; });
    void navigator.locks.request("sterm-agent-overlay", { mode: "shared" }, () => held)
      .catch((error) => console.error("悬浮窗后台状态保持失败", error));
  }
}
