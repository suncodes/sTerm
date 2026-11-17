export class PendingTerminalSequence {
  private state: "ground" | "escape" | "csi" | "string" | "stringEscape" = "ground";
  private pending = "";
  private osc = false;

  feed(text: string): void {
    for (const character of text) {
      const code = character.charCodeAt(0);
      if (character === "\x18" || character === "\x1a") { this.reset(); continue; }
      if (this.state === "ground") {
        if (character === "\x1b") { this.pending = character; this.state = "escape"; }
        continue;
      }
      this.pending += character;
      if (this.pending.length > 65536) this.pending = this.pending.slice(0, 65537);
      if (this.state === "string" || this.state === "stringEscape") {
        if (this.osc && character === "\x07" || this.state === "stringEscape" && character === "\\") { this.reset(); continue; }
        this.state = character === "\x1b" ? "stringEscape" : "string";
      } else if (character === "\x1b") {
        this.pending = character; this.state = "escape";
      } else if (this.state === "escape") {
        if (character === "[") this.state = "csi";
        else if ("]PX^_".includes(character)) { this.state = "string"; this.osc = character === "]"; }
        else if (code >= 0x30 && code <= 0x7e) this.reset();
      } else if (code >= 0x40 && code <= 0x7e) this.reset();
    }
  }

  snapshot(): string {
    if (this.pending.length > 65536) throw new Error("终端控制序列尚未结束，请稍后重新同步");
    return this.pending;
  }

  reset(): void { this.pending = ""; this.state = "ground"; this.osc = false; }
}
