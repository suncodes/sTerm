export interface ToastAction {
  label: string;
  primary?: boolean;
  onClick: () => void | Promise<void>;
}

let current: { el: HTMLElement; timer: number } | null = null;

/** 显示一条 toast（同一时间只保留一条，新的替换旧的） */
export function showToast(text: string, actions: ToastAction[] = [], timeoutMs = 20000): void {
  dismissToast();

  const el = document.createElement("div");
  el.className = "toast";

  const msg = document.createElement("div");
  msg.className = "toast-msg";
  msg.textContent = text;
  el.appendChild(msg);

  const bar = document.createElement("div");
  bar.className = "toast-actions";
  for (const a of actions) {
    const btn = document.createElement("button");
    btn.textContent = a.label;
    if (a.primary) btn.classList.add("primary");
    btn.onclick = () => {
      void Promise.resolve(a.onClick()).finally(() => dismissToast());
    };
    bar.appendChild(btn);
  }
  el.appendChild(bar);

  const close = document.createElement("button");
  close.className = "toast-close";
  close.textContent = "✕";
  close.title = "关闭";
  close.onclick = () => dismissToast();
  el.appendChild(close);

  // 底部倒计时进度条（与自动消失同步；--toast-ms 驱动 CSS 动画）
  const timerBar = document.createElement("div");
  timerBar.className = "toast-timer";
  timerBar.style.animationDuration = `${timeoutMs}ms`;
  el.appendChild(timerBar);

  document.getElementById("toasts")!.appendChild(el);
  const timer = window.setTimeout(dismissToast, timeoutMs);
  current = { el, timer };
}

export function dismissToast(): void {
  if (current) {
    clearTimeout(current.timer);
    current.el.remove();
    current = null;
  }
}
