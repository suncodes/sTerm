// sTerm 状态桥：有终端时发送 OSC 777，Windows 无控制终端时使用本地事件信箱。
import { randomUUID } from "node:crypto";
import { appendFileSync, closeSync, openSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const windows = process.platform === "win32";
const mailbox = windows && process.env.STERM_TAB_ID && process.env.LOCALAPPDATA
  ? join(process.env.LOCALAPPDATA, "sTerm", "events", process.env.STERM_TAB_ID + ".event")
  : undefined;

type AgentEvent = { messages?: Array<{ role: string; stopReason?: string }>; aborted?: boolean };

export default function (pi: { on: (event: string, handler: (event?: AgentEvent) => void) => void }): void {
  if (windows && !mailbox) return;
  let session = randomUUID();
  let failed = false;

  function emit(event: string): void {
    const frame = "\x1b]777;sterm;v=1;agent=pi;event=" + event + ";session=" + session + "\x07";
    let terminal: number | undefined;
    try {
      if (windows) {
        if (process.stdout.isTTY) process.stdout.write(frame);
        else if (mailbox) appendFileSync(mailbox, "pi-" + event + "\n");
        return;
      }
      terminal = openSync("/dev/tty", "w");
      writeFileSync(terminal, frame);
    } catch {
      // 无 PTY 或连接已关闭时不影响 Pi。
    } finally {
      if (terminal !== undefined) {
        try { closeSync(terminal); } catch { /* 终端可能已经关闭。 */ }
      }
    }
  }

  pi.on("session_start", () => {
    session = randomUUID();
    emit("start");
  });
  pi.on("agent_start", () => { failed = false; emit("prompt"); });
  pi.on("agent_end", event => {
    const last = event?.messages?.filter(message => message.role === "assistant").at(-1);
    failed = last?.stopReason === "error" || last?.stopReason === "aborted";
    // 旧版 Pi 仍可用此信号更新状态点。
    emit("stop");
  });
  pi.on("agent_settled", event => emit(event?.aborted || failed ? "error" : "complete"));
  pi.on("ui_prompt_start", () => emit("blocked"));
  pi.on("ui_prompt_end", () => emit("unblocked"));
  pi.on("session_shutdown", () => emit("end"));
}
