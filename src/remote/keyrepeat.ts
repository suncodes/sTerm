/** 先发送一次，按住一段时间后连发；取消后不再执行待发送的按键。 */
export class KeyRepeat {
  private timer: ReturnType<typeof setTimeout> | undefined;
  private version = 0;

  start(press: () => boolean): void {
    this.stop();
    const version = this.version;
    const schedule = (delay: number): void => {
      this.timer = setTimeout(() => {
        if (version !== this.version) return;
        const sent = press();
        if (version !== this.version) return;
        if (sent) schedule(75);
        else this.stop();
      }, delay);
    };
    if (press() && version === this.version) schedule(400);
  }

  stop(): void {
    this.version++;
    clearTimeout(this.timer);
    this.timer = undefined;
  }
}
