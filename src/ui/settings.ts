/**
 * 设置：双栏布局（左侧分类导航 + 右侧内容区），参照常规 shell 设置页。
 * - 外观：字号三区（标签栏 / 侧边栏 / 终端）
 *   标签栏高度随字号自适配（tab 高 = 栏高 - 8px 恒定边距）
 *   侧栏内部 em 相对缩放；终端字号立即生效并 refit
 * - 关于：版本信息
 * 持久化：字号使用 localStorage；悬浮窗设置由后端持久化；所有改动即时生效
 */

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { OverlaySnapshot } from "../overlay/types";

const KEY_TAB = "sterm-font-tab";
const KEY_SIDEBAR = "sterm-font-sidebar";
const KEY_TERM = "sterm-font-term";

export const FONT_DEFAULTS: FontSettings = { tab: 12, sidebar: 12, term: 13 };
const LIMIT: Record<keyof FontSettings, [number, number]> = {
  tab: [10, 18],
  sidebar: [10, 18],
  term: [8, 28],
};

export interface FontSettings {
  tab: number;
  sidebar: number;
  term: number;
}

function readNum(key: string, def: number, min: number, max: number): number {
  const v = Number(localStorage.getItem(key));
  return Number.isFinite(v) && v >= min && v <= max ? Math.round(v) : def;
}

export function loadFontSettings(): FontSettings {
  return {
    tab: readNum(KEY_TAB, FONT_DEFAULTS.tab, LIMIT.tab[0], LIMIT.tab[1]),
    sidebar: readNum(KEY_SIDEBAR, FONT_DEFAULTS.sidebar, LIMIT.sidebar[0], LIMIT.sidebar[1]),
    term: readNum(KEY_TERM, FONT_DEFAULTS.term, LIMIT.term[0], LIMIT.term[1]),
  };
}

function persist(f: FontSettings): void {
  localStorage.setItem(KEY_TAB, String(f.tab));
  localStorage.setItem(KEY_SIDEBAR, String(f.sidebar));
  localStorage.setItem(KEY_TERM, String(f.term));
}

/** 标签栏 / 侧栏布局自适配（CSS 变量） */
export function applyLayoutFonts(f: FontSettings): void {
  // 字号 → 栏高：10→37, 11→38, 12→40, 13→41, 14→43, 16→45, 18→48（上下共 8px 边距）
  const tabbarH = Math.max(37, Math.round(f.tab * 1.4) + 23);
  const root = document.documentElement.style;
  root.setProperty("--tabbar-h", `${tabbarH}px`);
  root.setProperty("--tab-h", `${tabbarH - 8}px`);
  root.setProperty("--tab-font", `${f.tab}px`);
  root.setProperty("--sidebar-font", `${f.sidebar}px`);
}

export interface SettingsCallbacks {
  /** 终端字号变化（含启动时） */
  onTermFont: (px: number) => void;
  shellId: string;
}

interface ShellIntegrationStatus {
  installed: boolean;
}

/* ---------- 小工具 ---------- */

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

export function openSettings(cb: SettingsCallbacks): void {
  if (document.querySelector(".dialog-root")) return;
  const f = loadFontSettings();

  const root = el("div", "dialog-root");
  const dlg = el("div", "settings-dialog");

  /* 标题栏 */
  const head = el("div", "settings-head");
  head.appendChild(el("span", "settings-head-title", "设置"));
  const closeBtn = el("button", "settings-close", "✕");
  closeBtn.title = "关闭";
  head.appendChild(closeBtn);
  dlg.appendChild(head);

  /* 双栏 */
  const body = el("div", "settings-body");
  const nav = el("div", "settings-nav");
  const content = el("div", "settings-content");
  body.append(nav, content);
  dlg.appendChild(body);
  root.appendChild(dlg);
  document.body.appendChild(root);

  let disposePage = (): void => {};
  const close = (): void => { disposePage(); root.remove(); };
  closeBtn.onclick = close;
  root.onclick = (e) => {
    if (e.target === root) close();
  };

  /* ---------- 页面：外观（字号） ---------- */

  const renderAppearance = (): HTMLElement => {
    const page = el("div");

    page.appendChild(el("div", "settings-section", "字号"));
    const apply = (): void => {
      persist(f);
      applyLayoutFonts(f);
      cb.onTermFont(f.term);
    };
    const syncers: (() => void)[] = [];

    for (const [key, label] of [
      ["tab", "标签栏"],
      ["sidebar", "侧边栏"],
      ["term", "终端"],
    ] as const) {
      const row = el("div", "settings-row");
      row.appendChild(el("span", "settings-label", label));

      const stepper = el("div", "settings-stepper");
      const minus = el("button", undefined, "−");
      minus.title = "减小";
      const val = el("span", "settings-value", String(f[key]));
      const plus = el("button", undefined, "＋");
      plus.title = "增大";

      const sync = (): void => {
        val.textContent = String(f[key]);
        minus.disabled = f[key] <= LIMIT[key][0];
        plus.disabled = f[key] >= LIMIT[key][1];
      };
      minus.onclick = () => {
        if (f[key] > LIMIT[key][0]) {
          f[key]--;
          sync();
          apply();
        }
      };
      plus.onclick = () => {
        if (f[key] < LIMIT[key][1]) {
          f[key]++;
          sync();
          apply();
        }
      };
      sync();
      syncers.push(sync);
      stepper.append(minus, val, plus);
      row.appendChild(stepper);
      page.appendChild(row);
    }

    const hint = el(
      "div",
      "settings-hint",
      "标签栏高度随字号自适应；终端字号立即生效并重新适配行列；侧栏按比例缩放。",
    );
    page.appendChild(hint);

    const reset = el("button", "dialog-btn settings-reset", "恢复默认字号");
    reset.onclick = () => {
      Object.assign(f, FONT_DEFAULTS);
      syncers.forEach((fn) => fn());
      apply();
    };
    page.appendChild(reset);
    return page;
  };

  /* ---------- 页面：悬浮窗 ---------- */

  const renderOverlay = (): HTMLElement => {
    const page = el("div");
    page.appendChild(el("div", "settings-section", "全局状态"));
    const row = el("label", "settings-row settings-toggle-row");
    const copy = el("div");
    copy.appendChild(el("div", "settings-label", "Agent 悬浮窗"));
    copy.appendChild(el("div", "settings-toggle-desc", "在其他软件上方显示 Agent 会话状态"));
    const input = el("input", "settings-switch");
    input.type = "checkbox";
    input.setAttribute("role", "switch");
    input.setAttribute("aria-label", "Agent 悬浮窗");
    input.disabled = true;
    row.append(copy, input);
    page.appendChild(row);
    page.appendChild(el("div", "settings-hint",
      "主窗口最小化后继续显示。拖动小窗顶部可移动，位置自动保存；点击会话返回对应标签。关闭小窗不会结束会话，退出 sTerm 后小窗同时关闭。"));
    page.appendChild(el("div", "settings-hint",
      "显示本应用内已识别的 Agent 会话，状态与标签栏一致。首次开启默认位于主屏右上方。"));
    page.appendChild(el("div", "settings-section settings-overlay-appearance", "外观"));
    const opacityRow = el("div", "settings-row settings-opacity-row");
    const opacityLabel = el("label", "settings-label", "底色透明度");
    opacityLabel.htmlFor = "overlay-transparency";
    const opacityControl = el("div", "settings-opacity-control");
    const opacityInput = el("input", "settings-range");
    opacityInput.type = "range";
    opacityInput.id = "overlay-transparency";
    opacityInput.min = "0";
    opacityInput.max = "100";
    opacityInput.step = "1";
    opacityInput.disabled = true;
    opacityInput.setAttribute("aria-describedby", "overlay-transparency-hint");
    const opacityValue = el("output", "settings-value");
    opacityValue.htmlFor.value = opacityInput.id;
    const syncOpacity = (transparency: number): void => {
      opacityInput.value = String(transparency);
      opacityInput.setAttribute("aria-valuetext", `${transparency}% 透明`);
      opacityValue.value = `${transparency}%`;
    };
    syncOpacity(20);
    opacityControl.append(opacityInput, opacityValue);
    opacityRow.append(opacityLabel, opacityControl);
    page.appendChild(opacityRow);
    const opacityHint = el("div", "settings-hint settings-opacity-hint",
      "数值越高，底色越透明；文字与状态点保持清晰。即时生效并自动保存，下次开启继续使用。");
    opacityHint.id = "overlay-transparency-hint";
    page.appendChild(opacityHint);
    const error = el("div", "settings-error");
    error.setAttribute("role", "status");
    page.appendChild(error);
    let disposed = false;
    let busy = false;
    let revision = -1;
    let unlisten: (() => void) | undefined;
    let latest: OverlaySnapshot | undefined;
    let pendingOpacity: number | undefined;
    let savingOpacity = false;
    let opacityTimer: number | undefined;
    const apply = (snapshot: OverlaySnapshot): void => {
      if (disposed || snapshot.revision < revision) return;
      revision = snapshot.revision;
      latest = snapshot;
      input.checked = snapshot.enabled;
      if (pendingOpacity === undefined && !savingOpacity) syncOpacity(100 - snapshot.background_opacity);
    };
    // 合并拖动产生的频繁写入；串行保存，保证最终值不会被较早的响应覆盖。
    const saveOpacity = async (): Promise<void> => {
      if (savingOpacity) return;
      savingOpacity = true;
      try {
        while (pendingOpacity !== undefined) {
          const opacity = pendingOpacity;
          pendingOpacity = undefined;
          try {
            apply(await invoke<OverlaySnapshot>("overlay_set_opacity", { opacity }));
          } catch (reason) {
            if (!disposed) error.textContent = "保存透明度失败：" + String(reason);
          }
        }
      } finally {
        savingOpacity = false;
        if (!disposed && latest) syncOpacity(100 - latest.background_opacity);
      }
    };
    opacityInput.oninput = () => {
      const transparency = Number(opacityInput.value);
      syncOpacity(transparency);
      error.textContent = "";
      pendingOpacity = 100 - transparency;
      if (opacityTimer === undefined) {
        opacityTimer = window.setTimeout(() => {
          opacityTimer = undefined;
          void saveOpacity();
        }, 80);
      }
    };
    opacityInput.onchange = () => {
      window.clearTimeout(opacityTimer);
      opacityTimer = undefined;
      void saveOpacity();
    };
    disposePage = () => {
      disposed = true;
      unlisten?.();
      window.clearTimeout(opacityTimer);
      void saveOpacity();
    };
    void (async () => {
      const stop = await listen<OverlaySnapshot>("overlay-changed", (e) => apply(e.payload));
      if (disposed) { stop(); return; }
      unlisten = stop;
      apply(await invoke<OverlaySnapshot>("overlay_snapshot"));
      if (!disposed) { input.disabled = false; opacityInput.disabled = false; }
    })().catch((reason) => {
      if (!disposed) error.textContent = "读取悬浮窗设置失败：" + String(reason);
    });
    input.onchange = async () => {
      if (busy) return;
      busy = true;
      input.disabled = true;
      error.textContent = "";
      const previous = !input.checked;
      try {
        apply(await invoke<OverlaySnapshot>("overlay_set_enabled", { enabled: input.checked }));
      } catch (reason) {
        if (!disposed) {
          input.checked = previous;
          error.textContent = "更新悬浮窗设置失败：" + String(reason);
        }
      } finally {
        busy = false;
        if (!disposed) { input.disabled = false; opacityInput.disabled = false; }
      }
    };
    return page;
  };

  /* ---------- 页面：集成 ---------- */

  const renderIntegrations = (): HTMLElement => {
    const page = el("div");
    page.appendChild(el("div", "settings-section", "PowerShell 目录上报"));
    page.appendChild(el("div", "settings-hint",
      "可选。安装后会在当前 PowerShell 的 $PROFILE 中加入一小段 OSC 7 代码，用于更准确地识别终端当前目录；其他 profile 内容保持不变。"));

    const shellStatus = el("div", "settings-hint");
    const shellActions = el("div", "settings-integration-actions");
    const installShell = el("button", "dialog-btn accent", "安装 PowerShell 集成");
    const removeShell = el("button", "dialog-btn", "移除 PowerShell 集成");
    shellActions.append(installShell, removeShell);
    page.append(shellStatus, shellActions);

    page.appendChild(el("div", "settings-section settings-integration-section", "Agent 状态"));
    page.appendChild(el("div", "settings-hint",
      "Claude 和 Codex 使用原生终端协议，无需写入配置。此操作仅清理旧版 sTerm Hook，并在本机已安装 Pi 时写入或更新 Pi 扩展；修改前会保留 .bak 备份。"));
    const installAgents = el("button", "dialog-btn accent", "安装 / 更新 Agent 集成");
    page.appendChild(installAgents);

    const result = el("div", "settings-error");
    result.setAttribute("role", "status");
    page.appendChild(result);

    let busy = false;
    const setBusy = (value: boolean): void => {
      busy = value;
      installShell.disabled = value;
      removeShell.disabled = value;
      installAgents.disabled = value;
    };
    const refreshShellStatus = async (): Promise<void> => {
      try {
        const status = await invoke<ShellIntegrationStatus>("shell_integration_status", { shellId: cb.shellId });
        shellStatus.textContent = status.installed
          ? "已安装。移除时只删除 sTerm 标记包围的代码块。"
          : "尚未安装。默认不修改 PowerShell profile。";
        installShell.disabled = busy;
        removeShell.disabled = busy || !status.installed;
      } catch (reason) {
        shellStatus.textContent = "当前默认 Shell 不支持 PowerShell 集成。";
        installShell.disabled = true;
        removeShell.disabled = true;
        result.textContent = String(reason);
      }
    };
    installShell.onclick = async () => {
      if (busy) return;
      setBusy(true);
      result.textContent = "";
      try {
        await invoke<ShellIntegrationStatus>("install_shell_integration", { shellId: cb.shellId });
        await refreshShellStatus();
      } catch (reason) {
        result.textContent = "安装 PowerShell 集成失败：" + String(reason);
      } finally {
        setBusy(false);
        void refreshShellStatus();
      }
    };
    removeShell.onclick = async () => {
      if (busy) return;
      setBusy(true);
      result.textContent = "";
      try {
        await invoke<ShellIntegrationStatus>("remove_shell_integration", { shellId: cb.shellId });
        await refreshShellStatus();
      } catch (reason) {
        result.textContent = "移除 PowerShell 集成失败：" + String(reason);
      } finally {
        setBusy(false);
        void refreshShellStatus();
      }
    };
    installAgents.onclick = async () => {
      if (busy) return;
      setBusy(true);
      result.textContent = "";
      try {
        for (const cli of ["claude", "codex", "pi"]) {
          await invoke("install_agent_integration", { cli });
        }
        result.className = "settings-hint";
        result.textContent = "Agent 集成已检查；请重启对应 Agent 进程后验证状态。";
      } catch (reason) {
        result.className = "settings-error";
        result.textContent = "更新 Agent 集成失败：" + String(reason);
      } finally {
        setBusy(false);
        void refreshShellStatus();
      }
    };
    void refreshShellStatus();
    return page;
  };

  /* ---------- 页面：关于 ---------- */

  const renderAbout = (): HTMLElement => {
    const page = el("div");
    page.appendChild(el("div", "settings-section", "关于"));

    const rows: [string, () => string][] = [
      ["名称", () => "sTerm"],
      ["定位", () => "轻量 Windows 本地终端 + AI Agent 工作台"],
      ["技术栈", () => "Tauri 2 · Rust ConPTY · xterm.js"],
      ["数据目录", () => "%APPDATA%\\io.github.suncodes.sterm"],
    ];
    for (const [k, v] of rows) {
      const row = el("div", "settings-row");
      row.appendChild(el("span", "settings-label", k));
      row.appendChild(el("span", "settings-value", v()));
      page.appendChild(row);
    }
    const verRow = el("div", "settings-row");
    verRow.appendChild(el("span", "settings-label", "版本"));
    const ver = el("span", "settings-value", "…");
    verRow.appendChild(ver);
    page.appendChild(verRow);
    void getVersion()
      .then((v) => (ver.textContent = v))
      .catch(() => (ver.textContent = "0.1.0"));
    return page;
  };

  /* ---------- 导航切换 ---------- */

  const pages: { id: string; label: string; render: () => HTMLElement }[] = [
    { id: "appearance", label: "外观", render: renderAppearance },
    { id: "overlay", label: "悬浮窗", render: renderOverlay },
    { id: "integrations", label: "集成", render: renderIntegrations },
    { id: "about", label: "关于", render: renderAbout },
  ];
  const items: HTMLElement[] = [];
  const select = (id: string): void => {
    disposePage();
    disposePage = () => {};
    for (const it of items) it.classList.toggle("active", it.dataset.page === id);
    content.replaceChildren(...pages.filter((pg) => pg.id === id).map((pg) => pg.render()));
  };
  for (const pg of pages) {
    const item = el("div", "settings-nav-item", pg.label);
    item.dataset.page = pg.id;
    item.onclick = () => select(pg.id);
    nav.appendChild(item);
    items.push(item);
  }
  select("appearance");
}
