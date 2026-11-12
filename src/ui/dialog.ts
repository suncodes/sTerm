/**
 * Win11 ContentDialog 风格的输入弹窗：
 * 半透明遮罩 + #2B2B2B 圆角 8px 卡片 + 右下角按钮（主按钮系统蓝）。
 * Enter 确认 / Esc 取消 / 点击遮罩取消。
 */

export interface DialogOptions {
  title: string;
  /** 说明文字（可选） */
  description?: string;
  placeholder?: string;
  defaultValue?: string;
  confirmText?: string;
  cancelText?: string;
  onConfirm: (value: string) => void | Promise<void>;
}

let open = false;

export interface ConfirmOptions {
  title: string;
  /** 正文说明（支持 \n 换行） */
  message: string;
  confirmText?: string;
  cancelText?: string;
  /** 危险操作：确认按钮红色 */
  danger?: boolean;
}

/** 二次确认弹窗，resolve(true) 表示用户确认 */
export function showConfirmDialog(opts: ConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    if (open) {
      resolve(false);
      return;
    }
    open = true;

    const root = document.createElement("div");
    root.className = "dialog-root";

    const card = document.createElement("div");
    card.className = "dialog-card";

    const title = document.createElement("div");
    title.className = "dialog-title";
    title.textContent = opts.title;
    card.appendChild(title);

    const desc = document.createElement("div");
    desc.className = "dialog-desc";
    desc.style.whiteSpace = "pre-line";
    desc.textContent = opts.message;
    card.appendChild(desc);

    const btns = document.createElement("div");
    btns.className = "dialog-buttons";

    const cancel = document.createElement("button");
    cancel.className = "dialog-btn";
    cancel.textContent = opts.cancelText ?? "取消";

    const ok = document.createElement("button");
    ok.className = "dialog-btn " + (opts.danger ? "danger" : "accent");
    ok.textContent = opts.confirmText ?? "确定";

    btns.append(cancel, ok);
    card.appendChild(btns);

    root.appendChild(card);
    document.body.appendChild(root);

    const finish = (result: boolean) => {
      open = false;
      root.remove();
      resolve(result);
    };

    ok.onclick = () => finish(true);
    cancel.onclick = () => finish(false);
    root.onclick = (e) => {
      if (e.target === root) finish(false);
    };
    card.onkeydown = (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        finish(true);
      } else if (e.key === "Escape") {
        e.preventDefault();
        finish(false);
      }
    };

    setTimeout(() => ok.focus(), 0);
  });
}

export function showInputDialog(opts: DialogOptions): void {
  if (open) return;
  open = true;

  const root = document.createElement("div");
  root.className = "dialog-root";

  const card = document.createElement("div");
  card.className = "dialog-card";

  const title = document.createElement("div");
  title.className = "dialog-title";
  title.textContent = opts.title;
  card.appendChild(title);

  if (opts.description) {
    const desc = document.createElement("div");
    desc.className = "dialog-desc";
    desc.textContent = opts.description;
    card.appendChild(desc);
  }

  const input = document.createElement("input");
  input.className = "dialog-input";
  input.placeholder = opts.placeholder ?? "";
  input.value = opts.defaultValue ?? "";
  input.spellcheck = false;
  card.appendChild(input);

  const btns = document.createElement("div");
  btns.className = "dialog-buttons";

  const cancel = document.createElement("button");
  cancel.className = "dialog-btn";
  cancel.textContent = opts.cancelText ?? "取消";

  const ok = document.createElement("button");
  ok.className = "dialog-btn accent";
  ok.textContent = opts.confirmText ?? "确定";

  btns.append(cancel, ok);
  card.appendChild(btns);

  root.appendChild(card);
  document.body.appendChild(root);

  const close = () => {
    open = false;
    root.remove();
  };

  let busy = false;
  const submit = () => {
    const v = input.value.trim();
    if (!v || busy) return;
    busy = true;
    void Promise.resolve(opts.onConfirm(v)).finally(() => close());
  };

  ok.onclick = submit;
  cancel.onclick = close;
  root.onclick = (e) => {
    if (e.target === root) close();
  };
  input.onkeydown = (e) => {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  setTimeout(() => {
    input.focus();
    input.select();
  }, 0);
}
