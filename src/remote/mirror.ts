interface MirrorFrame {
  data: string;
  cols: number;
  rows: number;
  snapshot: boolean;
}

interface MirrorTerminal {
  reset(): void;
  write(data: string, callback: () => void): void;
}

export class TerminalMirror {
  private epoch = 0;
  private queue: Promise<void> = Promise.resolve();

  constructor(private readonly terminal: MirrorTerminal, private readonly resize: (cols: number, rows: number) => void) {}

  invalidate(clear = false): void {
    const epoch = ++this.epoch;
    if (clear) this.queue = this.queue.then(() => { if (epoch === this.epoch) this.terminal.reset(); });
  }

  write(frame: MirrorFrame, onParsed: () => void): void {
    const epoch = this.epoch;
    this.queue = this.queue.then(async () => {
      if (epoch !== this.epoch) return;
      if (frame.snapshot) this.terminal.reset();
      this.resize(frame.cols, frame.rows);
      await new Promise<void>(resolve => this.terminal.write(frame.data, resolve));
      if (epoch === this.epoch) onParsed();
    });
  }

  async flush(): Promise<void> { await this.queue; }
}
