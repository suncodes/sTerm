import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "../ui/toast";
import type { FeishuSettings, FeishuStatus } from "./feishu";

// 设置页可能在保存期间重新创建，读取前等待本次写入。
let pendingSave: Promise<void> | undefined;

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, content?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

export function renderFeishuSettings(): { page: HTMLElement; dispose: () => void } {
  const page = element("section", "settings-feishu");
  const form = element("form", "feishu-form");
  const heading = element("div", "settings-section", "飞书机器人");
  const enabled = element("input", "settings-switch");
  enabled.type = "checkbox";
  const enabledRow = element("label", "settings-row settings-toggle-row", "启用飞书通知");
  enabledRow.append(enabled);

  const field = (name: string, placeholder: string, type = "text"): HTMLInputElement => {
    const label = element("label", "feishu-field", name);
    const input = element("input", "feishu-input");
    input.type = type;
    input.placeholder = placeholder;
    input.autocomplete = "off";
    input.spellcheck = false;
    label.append(input);
    form.append(label);
    return input;
  };

  const appId = field("App ID", "cli_…");
  const appSecret = field("App Secret", "输入应用密钥", "password");
  const chatId = field("目标群 Chat ID", "oc_…");
  const allowedUsers = field("续问用户 Open ID（可选）", "ou_…，多个用户用逗号或空格分隔");
  const allowAllUsers = element("input", "settings-switch");
  allowAllUsers.type = "checkbox";
  const allowAllUsersRow = element("label", "settings-row settings-toggle-row", "允许群内所有用户续问");
  allowAllUsersRow.append(allowAllUsers);
  const hint = element("p", "feishu-hint", "清空 Open ID 后可开启全员续问。名单为空且开关关闭时只发通知。续问时请在完成通知的话题中 @ 机器人。");
  const sendTitle = element("input", "settings-switch");
  sendTitle.type = "checkbox";
  const titleRow = element("label", "settings-row settings-toggle-row", "在卡片显示终端标签名");
  titleRow.append(sendTitle);
  const status = element("p", "feishu-status", "正在读取设置…");
  status.setAttribute("role", "status");
  const feedback = element("p", "feishu-feedback");
  feedback.setAttribute("role", "status");
  const actions = element("div", "feishu-actions");
  const save = element("button", "settings-integration-btn", "保存");
  save.type = "submit";
  const test = element("button", "settings-integration-btn", "发送测试通知");
  test.type = "button";
  actions.append(save, test);
  form.append(allowAllUsersRow, hint, titleRow, status, actions, feedback);
  page.append(heading, enabledRow, form);

  let disposed = false;
  let busy = false;
  let saved: FeishuSettings | undefined;
  let unlisten: (() => void) | undefined;
  const controls = [enabled, appId, appSecret, chatId, allowedUsers, allowAllUsers, sendTitle, save, test];

  function readAllowedUsers(): string[] {
    return allowedUsers.value.split(/[\s,，]+/).filter(Boolean);
  }

  function updateControls(): void {
    for (const input of controls) input.disabled = busy || !saved;
    if (readAllowedUsers().length > 0) {
      allowAllUsers.checked = false;
      allowAllUsers.disabled = true;
    }
    test.disabled ||= !saved?.status.enabled;
  }

  function showStatus(value: FeishuStatus): void {
    if (disposed || !saved) return;
    saved.status = value;
    status.textContent = !value.enabled
      ? value.connectionError ?? "飞书通知已关闭"
      : value.connected ? "长连接已就绪" : value.connectionError ?? "正在连接飞书…";
    if (value.deliveryError) status.textContent += " · " + value.deliveryError;
    status.classList.toggle("error", Boolean(value.connectionError || value.deliveryError));
    updateControls();
  }

  async function load(keepEdits = false): Promise<void> {
    await pendingSave?.catch(() => {});
    if (disposed) return;
    const value = await invoke<FeishuSettings>("feishu_settings");
    if (disposed) return;
    saved = value;
    enabled.checked = value.status.enabled;
    if (!keepEdits) {
      appId.value = value.appId;
      appSecret.value = "";
      appSecret.placeholder = value.hasSecret ? "已保存，留空保留" : "输入应用密钥";
      chatId.value = value.chatId;
      allowedUsers.value = value.allowedUsers.join(", ");
      allowAllUsers.checked = value.allowAllUsers;
      sendTitle.checked = value.sendTitle;
    }
    showStatus(value.status);
  }

  function run(action: () => Promise<void>, label: string): void {
    if (busy || disposed) return;
    busy = true;
    updateControls();
    feedback.textContent = "";
    feedback.classList.remove("error");
    void action().then(() => {
      if (!disposed) feedback.textContent = label;
    }).catch(error => {
      if (disposed) return;
      feedback.textContent = String(error);
      feedback.classList.add("error");
    }).finally(() => {
      busy = false;
      if (!disposed) updateControls();
    });
  }

  function saveSettings(toggleOnly = false): void {
    const current = saved;
    if (!current) return;
    run(async () => {
      const operation = toggleOnly
        ? invoke<void>("feishu_set_enabled", { enabled: enabled.checked })
        : invoke<void>("feishu_save", { update: {
          enabled: enabled.checked, appId: appId.value, appSecret: appSecret.value.trim() || null,
          chatId: chatId.value, allowedUsers: readAllowedUsers(),
          allowAllUsers: allowAllUsers.checked, sendTitle: sendTitle.checked,
        } });
      pendingSave = operation;
      try {
        await operation;
      } catch (error) {
        if (disposed) showToast("保存飞书设置失败：" + String(error));
        else enabled.checked = current.status.enabled;
        throw error;
      } finally {
        pendingSave = undefined;
      }
      await load(toggleOnly);
    }, "设置已保存");
  }

  allowedUsers.oninput = updateControls;
  enabled.onchange = () => saveSettings(true);
  form.onsubmit = event => {
    event.preventDefault();
    saveSettings();
  };
  test.onclick = () => run(() => invoke("feishu_test"), "测试通知已发到已保存的目标群");
  updateControls();
  void listen<FeishuStatus>("feishu-status", event => showStatus(event.payload)).then(stop => {
    if (disposed) stop(); else unlisten = stop;
  }).catch(() => {});
  void load().catch(error => {
    if (disposed) return;
    status.textContent = String(error);
    status.classList.add("error");
  });
  return { page, dispose: () => {
    disposed = true;
    appSecret.value = "";
    unlisten?.();
  } };
}
