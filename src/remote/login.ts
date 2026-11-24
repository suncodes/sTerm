export interface AuthStatus {
  authenticated: boolean;
  passwordEnabled: boolean;
  passwordAllowed: boolean;
}

export async function pairRemoteBrowser(token: string, notification: boolean): Promise<void> {
  const response = await fetch("/api/auth", { credentials: "same-origin", cache: "no-store" });
  if (!response.ok) throw new Error("无法检查访问授权，请稍后重试。");
  const status: AuthStatus = await response.json();
  if (status.authenticated || (notification && status.passwordEnabled)) return;
  const paired = await fetch("/api/pair", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }), credentials: "same-origin",
  });
  if (!paired.ok) throw new Error(notification
    ? "临时配对链接已失效或已使用。请在对应飞书话题中 @ 机器人发送「打开会话」获取新链接。"
    : "配对二维码已失效或已使用，请在电脑端重新生成，或输入访问码。");
}

export class RemoteLogin {
  private readonly form: HTMLFormElement;
  private readonly password: HTMLInputElement;
  private readonly submit: HTMLButtonElement;
  private readonly message: HTMLElement;
  private readonly retry: HTMLButtonElement;
  private generation = 0;
  private blockedUntil = 0;
  private cooldown: number | undefined;

  constructor(private readonly root: HTMLElement, private readonly authenticated: () => void) {
    this.form = root.querySelector("#login-form")!;
    this.password = root.querySelector("#login-password")!;
    this.submit = root.querySelector("#login-submit")!;
    this.message = root.querySelector("#login-message")!;
    this.retry = root.querySelector("#login-retry")!;
    this.form.onsubmit = event => { event.preventDefault(); void this.login(); };
    const show = root.querySelector<HTMLButtonElement>("#login-show")!;
    show.onclick = () => {
      const visible = this.password.type === "password";
      this.password.type = visible ? "text" : "password";
      show.textContent = visible ? "隐藏" : "显示";
      show.setAttribute("aria-pressed", String(visible));
    };
    this.retry.onclick = () => void this.check();
  }

  hide(): void {
    this.generation++;
    this.root.hidden = true;
    this.retry.disabled = false;
    this.password.value = "";
    this.password.type = "password";
    const show = this.root.querySelector<HTMLButtonElement>("#login-show")!;
    show.textContent = "显示";
    show.setAttribute("aria-pressed", "false");
    window.clearTimeout(this.cooldown);
  }

  async check(message = ""): Promise<void> {
    const generation = ++this.generation;
    this.root.hidden = false;
    this.form.hidden = true;
    this.retry.hidden = true;
    this.retry.disabled = false;
    this.message.textContent = "正在检查访问授权…";
    try {
      const response = await fetch("/api/auth", { credentials: "same-origin", cache: "no-store" });
      if (!response.ok) throw new Error("暂时无法连接电脑，请确认远程访问和公网连接已开启。");
      const status: AuthStatus = await response.json();
      if (generation !== this.generation) return;
      if (status.authenticated) { this.hide(); this.authenticated(); return; }
      this.form.hidden = !status.passwordEnabled || !status.passwordAllowed;
      this.retry.hidden = false;
      this.message.textContent = message || (!status.passwordEnabled
        ? "请在电脑端扫码配对，或启用访问码登录。飞书通知链接失效时，可在对应话题中 @ 机器人发送「打开会话」。"
        : !status.passwordAllowed ? "访问码登录需使用 HTTPS 公网地址；当前地址请扫码配对。"
        : "输入电脑端设置的 6 位数字访问码。");
      this.updateCooldown();
    } catch (error) {
      if (generation !== this.generation) return;
      this.retry.hidden = false;
      this.message.textContent = error instanceof Error ? error.message : "无法连接电脑，请稍后重试。";
    }
  }

  private updateCooldown(): void {
    window.clearTimeout(this.cooldown);
    const wait = Math.max(0, Math.ceil((this.blockedUntil - Date.now()) / 1000));
    this.submit.disabled = wait > 0;
    this.submit.textContent = wait ? "稍后重试" : "进入";
    if (wait) {
      this.message.textContent = "尝试次数较多，请在 " + wait + " 秒后重试。";
      this.cooldown = window.setTimeout(() => this.updateCooldown(), 1000);
    } else if (this.blockedUntil) {
      this.blockedUntil = 0;
      this.message.textContent = "可以重新输入访问码。";
    }
  }

  private async login(): Promise<void> {
    if (this.submit.disabled || !this.form.reportValidity()) return;
    const generation = this.generation;
    this.submit.disabled = true;
    this.submit.textContent = "正在登录…";
    this.retry.disabled = true;
    this.message.textContent = "";
    const password = this.password.value;
    this.password.value = "";
    try {
      const response = await fetch("/api/password/login", {
        method: "POST", credentials: "same-origin", cache: "no-store",
        headers: { "Content-Type": "application/json" }, body: JSON.stringify({ password }),
      });
      if (generation !== this.generation) return;
      if (response.ok) { this.hide(); this.authenticated(); return; }
      const error = await response.json().catch(() => ({})) as { message?: string; retryAfter?: number };
      if (generation !== this.generation) return;
      this.message.textContent = error.message || "无法登录，请检查访问码后重试。";
      if (response.status === 429 && error.retryAfter) {
        this.blockedUntil = Date.now() + Math.min(3600, Math.max(1, error.retryAfter)) * 1000;
      }
    } catch {
      if (generation === this.generation) this.message.textContent = "连接失败，请确认电脑在线后重试。";
    } finally {
      if (generation === this.generation) {
        this.retry.disabled = false;
        this.updateCooldown();
        this.password.focus();
      }
    }
  }
}
