// sTerm 远端状态桥：Pi 生命周期事件 → 控制终端 OSC 777。
import { randomUUID } from "node:crypto";
import { closeSync, openSync, writeFileSync } from "node:fs";

let session = randomUUID();

function emit(event: string): void {
  let terminal: number | undefined;
  try {
    terminal = openSync("/dev/tty", "w");
    writeFileSync(terminal, "\x1b]777;sterm;v=1;agent=pi;event=" + event + ";session=" + session + "\x07");
  } catch {
    // 无 PTY 或连接已关闭时不影响 Pi。
  } finally {
    if (terminal !== undefined) {
      try { closeSync(terminal); } catch { /* 终端可能已经关闭。 */ }
    }
  }
}

export default function (pi: { on: (event: string, handler: () => void) => void }): void {
  pi.on("session_start", () => {
    session = randomUUID();
    emit("start");
  });
  pi.on("agent_start", () => emit("prompt"));
  pi.on("agent_end", () => emit("stop"));
  pi.on("session_shutdown", () => emit("end"));
}
