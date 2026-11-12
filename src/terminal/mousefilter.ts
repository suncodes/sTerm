/** 鼠标交互统一由终端前端处理，保留其余 VT 模式及输出顺序。 */
const MOUSE_MODES = new Set([9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]);

export class MouseModeFilter {
  private decoder = new TextDecoder();
  private pending = "";

  filter(data: Uint8Array): string {
    let text = this.pending + this.decoder.decode(data, { stream: true });
    // PTY 可在 UTF-8 字符、ESC 或 CSI 参数中间分块，留待下一批补齐。
    const tail = text.match(/(?:\x1b(?:\[[0-?]*)?|\x9b[0-?]*)$/);
    this.pending = tail?.[0] ?? "";
    if (this.pending) text = text.slice(0, -this.pending.length);

    return text.replace(/(\x1b\[|\x9b)\?([0-9:;]*)([hl])/g, (sequence, prefix, params: string, final) => {
      const modes = params.split(";");
      const kept = modes.filter((mode) => !MOUSE_MODES.has(Number(mode.split(":", 1)[0])));
      if (kept.length === modes.length) return sequence;
      // 空参数改为无作用的模式 0，保留 CSI 对解析器状态的正常终止行为。
      return `${prefix}?${kept.length ? kept.join(";") : "0"}${final}`;
    });
  }

  finish(): string {
    const text = this.pending + this.decoder.decode();
    this.pending = "";
    return text;
  }

  reset(): void {
    this.decoder = new TextDecoder();
    this.pending = "";
  }
}
