import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import QRCode from "qrcode";
import { remoteIcon } from "./icons";
import { resolveRemotePermission, type RemotePolicy } from "./permissions";
import type { RemoteController } from "./controller";
import type { TunnelConfig, TunnelStatus } from "./types";
import "./settings.css";

export function renderRemoteSettings(controller: RemoteController): { page: HTMLElement; dispose: () => void } {
  const page = document.createElement("div");
  page.className = "remote-settings";
  page.innerHTML = `
    <label class="rs-heading"><span><span class="rs-title">远程访问</span><span class="rs-description">通过手机或浏览器，继续本机的终端会话</span></span><span class="rs-heading-toggle"><span data-el="state" class="rs-state"></span><input data-el="enabled" class="settings-switch" type="checkbox" aria-label="远程访问总开关"></span></label>
    <div class="rs-tabs" role="tablist" aria-label="远程设置"><button data-tab="connection" role="tab" aria-selected="true">连接</button><button data-tab="permissions" role="tab" aria-selected="false" tabindex="-1">会话权限</button><button data-tab="public" role="tab" aria-selected="false" tabindex="-1">公网</button></div>
    <div data-panel="connection" role="tabpanel">
      <div data-el="off" class="rs-empty">${remoteIcon("remote")}<strong>开启后即可扫码连接</strong><p>先设置默认会话权限，也可以稍后再配置公网。</p><button data-el="start" class="settings-integration-btn rs-primary">开启远程访问</button></div>
      <div data-el="on" hidden>
        <div class="rs-kind"><div class="rs-segment" aria-label="连接地址类型"><button data-address="lan" aria-pressed="true">局域网</button><button data-address="public" aria-pressed="false">公网</button></div><span data-el="address-hint" class="rs-muted"></span></div>
        <div data-el="unavailable" class="rs-unavailable" hidden>${remoteIcon("globe")}<div>公网尚未连接<p>完成一次配置后，即可在外网扫码访问。</p><button data-el="go-public" class="rs-link">前往公网设置 →</button></div></div>
        <div data-el="pairing" class="rs-pairing"><div class="rs-qr-column"><div class="rs-qr-frame"><img data-el="qr" alt="sTerm 一次性配对二维码" hidden><span data-el="qr-placeholder">正在生成…</span></div><span class="rs-muted rs-qr-caption">手机相机扫码配对</span></div><div class="rs-pairing-details"><label class="rs-small-label" for="remote-address">连接地址</label><div class="rs-copy-field"><select id="remote-address" data-el="address" aria-label="连接地址"></select><button data-el="copy" class="rs-icon-button" aria-label="复制配对链接" title="复制配对链接">${remoteIcon("copy")}</button></div><p class="rs-muted rs-pair-hint">二维码含一次性配对凭证，5 分钟内有效。<br>已连接设备不受二维码刷新影响。</p><button data-el="refresh-qr" class="settings-integration-btn">${remoteIcon("refresh")}刷新二维码</button><p data-el="expiry" class="rs-quiet-detail"></p></div></div>
        <div class="rs-summary"><span><span class="rs-dot green"></span><strong data-el="shared-count">0</strong> 个会话已分享</span><span>已连接设备 <strong data-el="clients">0</strong></span><button data-el="manage" class="rs-link">管理</button></div>
        <div data-el="devices" class="rs-device" hidden><span>撤销已配对设备，终端继续运行。</span><button data-el="revoke" class="settings-integration-btn">撤销全部设备</button></div>
      </div>
      <details class="rs-advanced"><summary>高级选项</summary><label class="settings-row"><span>监听端口</span><input data-el="port" class="rs-field rs-port" type="number" min="1024" max="65535" aria-label="监听端口"></label><label class="settings-row"><span>允许局域网连接</span><input data-el="lan" class="settings-switch" type="checkbox"></label><p class="rs-muted">监听设置将在下次开启远程访问时生效。</p></details>
    </div>
    <div data-panel="permissions" role="tabpanel" hidden>
      <label class="settings-row rs-default"><span><span class="settings-label">默认分享会话</span><span class="rs-description">新会话自动出现在已配对的浏览器中</span></span><input data-el="share" class="settings-switch" type="checkbox"></label>
      <label class="settings-row rs-default"><span><span class="settings-label">默认允许输入</span><span class="rs-description">手机主动接管后，可发送文字和快捷键</span></span><input data-el="input" class="settings-switch" type="checkbox"></label>
      <p class="rs-inherit-hint">默认设置即时应用于所有「跟随默认」的会话。</p><div class="rs-session-heading">当前会话 <span data-el="session-count" class="rs-muted"></span></div><div data-el="sessions"></div><p class="settings-hint rs-footnote">${remoteIcon("info")}允许输入后，电脑仍可随时收回控制权。</p>
    </div>
    <div data-panel="public" role="tabpanel" hidden>
      <div class="rs-public-heading"><span class="settings-label">公网连接</span><span data-el="tunnel-state" class="rs-state off">未连接</span></div><div class="rs-segment rs-providers"><button data-provider="cloudflare" aria-pressed="true">Cloudflare Tunnel</button><button data-provider="ngrok" aria-pressed="false">ngrok</button></div><p data-el="provider-hint" class="rs-provider-hint"></p>
      <label class="rs-field-label" for="remote-token" data-el="token-label">Tunnel Token</label><input id="remote-token" data-el="token" class="rs-field rs-full" type="password" autocomplete="new-password">
      <div data-el="domain-row"><label class="rs-field-label" for="remote-domain">公网域名</label><input id="remote-domain" data-el="domain" class="rs-field rs-full" placeholder="terminal.example.com" autocomplete="off"></div>
      <div class="rs-public-actions"><button data-el="connect" class="settings-integration-btn rs-primary">连接 Cloudflare</button><button data-el="save" class="settings-integration-btn">保存配置</button><button data-el="cleanup" class="settings-integration-btn" hidden>清理遗留连接</button></div><p data-el="tunnel-message" class="rs-muted rs-tunnel-message">未连接时仍可使用局域网二维码</p>
      <details data-el="help" class="rs-advanced"><summary data-el="help-title">首次使用 Cloudflare</summary><ol data-el="help-steps"></ol></details>
      <details data-el="auto-setup" class="rs-advanced"><summary>自动创建 Cloudflare 隧道与 DNS</summary><p class="rs-muted">需有已激活域名。API Token 需具备 Zone Read、DNS Edit、Cloudflare Tunnel Edit 权限；仅创建新记录。</p><label class="rs-field-label" for="remote-api-token">Cloudflare API Token</label><input id="remote-api-token" data-el="api-token" class="rs-field rs-full" type="password" autocomplete="new-password"><label class="rs-field-label" for="remote-hostname">专用主机名</label><input id="remote-hostname" data-el="hostname" class="rs-field rs-full" placeholder="terminal.example.com"><button data-el="setup" class="settings-integration-btn">创建隧道与 DNS</button></details>
    </div><div data-el="result" class="settings-error" role="status"></div>`;
  const find = <ElementType extends HTMLElement>(name: string): ElementType => page.querySelector(`[data-el="${name}"]`) as ElementType;
  let disposed = false;
  let busy = false;
  let refreshing = false;
  let provider: "cloudflare" | "ngrok" = "cloudflare";
  let addressKind = "lan";
  let origin = "";
  let pairUrl = "";
  let qrVersion = 0;
  let qrTimer: number | undefined;
  let sessionsKey = "";
  let tunnel: TunnelStatus = { running: false, connected: false, message: "" };
  let saved: TunnelConfig | undefined;
  const port = find<HTMLInputElement>("port");
  const lan = find<HTMLInputElement>("lan");
  const token = find<HTMLInputElement>("token");
  const domain = find<HTMLInputElement>("domain");
  const address = find<HTMLSelectElement>("address");
  port.value = localStorage.getItem("sterm-remote-port") ?? "8765";
  lan.checked = localStorage.getItem("sterm-remote-lan") !== "false";
  const report = (reason: unknown): void => { if (!disposed) find("result").textContent = String(reason); };
  const selectTab = (name: string, focus = false): void => {
    page.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach(button => {
      const selected = button.dataset.tab === name;
      button.setAttribute("aria-selected", String(selected));
      button.tabIndex = selected ? 0 : -1;
      if (selected && focus) button.focus();
    });
    page.querySelectorAll<HTMLElement>("[data-panel]").forEach(panel => { panel.hidden = panel.dataset.panel !== name; });
  };
  const clearQr = (): void => {
    qrVersion++;
    window.clearTimeout(qrTimer);
    pairUrl = "";
    const image = find<HTMLImageElement>("qr");
    image.hidden = true;
    image.removeAttribute("src");
    find("qr-placeholder").hidden = false;
    find("qr-placeholder").textContent = "正在生成…";
    find("expiry").textContent = "";
    find<HTMLButtonElement>("copy").disabled = true;
  };
  const generateQr = async (): Promise<void> => {
    clearQr();
    const version = qrVersion;
    const selectedOrigin = origin;
    if (!controller.status || !selectedOrigin) return;
    try {
      const pair = await invoke<{ url: string; expiresIn: number }>("remote_pair", { origin: selectedOrigin });
      const data = await QRCode.toDataURL(pair.url, { width: 180, margin: 2, errorCorrectionLevel: "M" });
      if (disposed || version !== qrVersion || origin !== selectedOrigin) return;
      const image = find<HTMLImageElement>("qr");
      image.src = data;
      image.hidden = false;
      find("qr-placeholder").hidden = true;
      pairUrl = pair.url;
      find<HTMLButtonElement>("copy").disabled = false;
      const expiry = new Date(Date.now() + pair.expiresIn * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
      find("expiry").textContent = `有效至 ${expiry}`;
      qrTimer = window.setTimeout(() => { clearQr(); find("qr-placeholder").textContent = "二维码已过期"; }, pair.expiresIn * 1000);
    } catch (reason) {
      if (disposed || version !== qrVersion) return;
      find("qr-placeholder").textContent = "生成失败，请重试";
      report(reason);
    }
  };
  const operation = async (action: () => Promise<void>): Promise<void> => {
    if (busy || disposed) return;
    busy = true;
    find("result").textContent = "";
    render();
    try { await action(); } catch (reason) { report(reason); }
    finally { busy = false; if (!disposed) { await refresh(); render(); } }
  };
  const render = (): void => {
    if (disposed) return;
    const status = controller.status;
    find<HTMLInputElement>("enabled").checked = Boolean(status);
    find<HTMLInputElement>("enabled").disabled = busy;
    find("state").textContent = status ? "已开启" : "已关闭";
    find("state").classList.toggle("off", !status);
    find("off").hidden = Boolean(status);
    find("on").hidden = !status;
    port.disabled = Boolean(status) || busy;
    lan.disabled = Boolean(status) || busy;
    find<HTMLInputElement>("share").checked = controller.defaults.share;
    find<HTMLInputElement>("input").checked = controller.defaults.input;
    find<HTMLInputElement>("share").disabled = busy;
    find<HTMLInputElement>("input").disabled = busy || !controller.defaults.share;
    find("clients").textContent = String(status?.clients ?? 0);
    find("shared-count").textContent = String(controller.sessions.filter(session => session.isRemoteShared).length);
    find<HTMLButtonElement>("revoke").disabled = busy;
    find<HTMLButtonElement>("start").disabled = busy;
    const origins = (status?.origins ?? []).filter(value => addressKind === "public" ? value.startsWith("https:") : value.startsWith("http:"));
    const previous = origin;
    origin = origins.includes(origin) ? origin : origins.find(value => !value.includes("127.0.0.1") && !value.includes("localhost")) ?? origins[0] ?? "";
    if (JSON.stringify([...address.options].map(option => option.value)) !== JSON.stringify(origins)) address.replaceChildren(...origins.map(value => new Option(value, value)));
    address.value = origin;
    find("pairing").hidden = !origin;
    find("unavailable").hidden = Boolean(origin);
    find("address-hint").textContent = addressKind === "lan" ? "手机与电脑连接同一网络" : "通过 HTTPS 访问，无需在同一网络";
    page.querySelectorAll<HTMLButtonElement>("[data-address]").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.address === addressKind)));
    if (origin !== previous) { if (origin) void generateQr(); else clearQr(); }
    const key = JSON.stringify(controller.sessions.map(session => [session.id, session.title, session.isRunning, session.isRemotePaused, session.agentState.status, controller.policy(session), controller.defaults, busy]));
    if (key !== sessionsKey) {
      sessionsKey = key;
      find("session-count").textContent = String(controller.sessions.length);
      const list = find("sessions");
      list.replaceChildren();
      if (!controller.sessions.length) { const hint = document.createElement("p"); hint.className = "settings-hint"; hint.textContent = "暂无会话；新建终端后会自动使用默认权限。"; list.append(hint); }
      for (const session of controller.sessions) {
        const row = document.createElement("div"); row.className = "rs-session";
        const dot = document.createElement("span"); dot.className = `rs-dot ${session.agentState.status === "running" ? "gold" : session.agentState.status === "idle" ? "green" : ""}`;
        const description = document.createElement("span"); description.className = "rs-session-description";
        const name = document.createElement("span"); name.className = "rs-session-name"; name.textContent = session.title; name.title = session.title;
        const detail = document.createElement("small");
        const policy = controller.policy(session);
        const permission = resolveRemotePermission(policy, controller.defaults);
        const effective = !session.isRunning ? "未运行" : !permission.share ? "未分享" : session.isRemotePaused ? "已暂停分享，请重新选择权限恢复" : permission.input ? "允许接管" : "只读";
        detail.textContent = `${policy === "inherit" ? "跟随默认" : "单独设置"} · ${effective}`;
        description.append(name, detail);
        const select = document.createElement("select");
        select.dataset.session = String(session.id);
        select.setAttribute("aria-label", `${session.title} 的远程权限`);
        for (const [value, label] of [["inherit", "跟随默认"], ["off", "不分享"], ["readonly", "只读"], ["control", "允许接管"]]) select.add(new Option(label, value));
        select.value = policy; select.disabled = busy;
        select.onchange = () => void operation(() => controller.setPolicy(session, select.value as RemotePolicy));
        row.append(dot, description, select); list.append(row);
      }
    }
    const isCloudflare = provider === "cloudflare";
    page.querySelectorAll<HTMLButtonElement>("[data-provider]").forEach(button => { button.setAttribute("aria-pressed", String(button.dataset.provider === provider)); button.disabled = busy || tunnel.running; });
    find("cleanup").hidden = isCloudflare;
    find<HTMLButtonElement>("cleanup").disabled = busy || tunnel.running;
    find("domain-row").hidden = !isCloudflare;
    find("auto-setup").hidden = !isCloudflare;
    find("provider-hint").textContent = isCloudflare ? "适合日常使用，通过自己的域名访问。" : "适合快速测试，连接后自动获取 ngrok 公网地址。";
    find("token-label").textContent = isCloudflare ? "Tunnel Token" : "Authtoken";
    const credentialSaved = isCloudflare ? saved?.cloudflareReady : saved?.ngrokReady;
    token.placeholder = credentialSaved ? "凭证已保存，留空继续使用" : isCloudflare ? "粘贴 Cloudflare 的隧道令牌" : "粘贴 ngrok Authtoken";
    find("tunnel-state").textContent = tunnel.connected ? "已连接" : tunnel.running ? "连接中…" : "未连接";
    find("tunnel-state").classList.toggle("off", !tunnel.connected);
    find<HTMLButtonElement>("connect").disabled = busy || !status;
    find("connect").textContent = tunnel.running ? "断开连接" : `连接 ${isCloudflare ? "Cloudflare" : "ngrok"}`;
    find<HTMLButtonElement>("save").disabled = busy || tunnel.running;
    find<HTMLButtonElement>("setup").disabled = busy || !status || tunnel.running;
    token.disabled = busy || tunnel.running;
    domain.disabled = busy || tunnel.running;
    find("tunnel-message").textContent = tunnel.message || (!status ? "可先保存配置，开启远程访问后再连接公网。" : "未连接时仍可使用局域网二维码");
    find("help-title").textContent = `首次使用 ${isCloudflare ? "Cloudflare" : "ngrok"}`;
    const steps = isCloudflare ? ["将自己的域名接入 Cloudflare。", "在 Zero Trust 中创建 Tunnel，复制 Token。", `添加公网主机名，指向 http://127.0.0.1:${status?.port ?? port.value}。`] : ["注册 ngrok 账号，在控制台获取 Authtoken。", "粘贴凭证后连接，首次按需下载官方客户端。", "免费套餐首次访问可能需要通过 ngrok 提示页。"];
    find("help-steps").replaceChildren(...steps.map(text => { const item = document.createElement("li"); item.textContent = text; return item; }));
  };
  const refresh = async (): Promise<void> => {
    if (disposed || refreshing) return;
    refreshing = true;
    try {
      tunnel = await invoke<TunnelStatus>("tunnel_status");
      if (tunnel.running && tunnel.provider) provider = tunnel.provider;
      await controller.refresh();
    } catch (reason) { report(reason); }
    finally { refreshing = false; render(); }
  };
  const start = async (): Promise<void> => {
    const selectedPort = Number(port.value);
    if (!Number.isInteger(selectedPort) || selectedPort < 1024 || selectedPort > 65535) throw new Error("端口应为 1024–65535 的整数");
    localStorage.setItem("sterm-remote-port", String(selectedPort));
    localStorage.setItem("sterm-remote-lan", String(lan.checked));
    await controller.start(selectedPort, lan.checked);
  };
  const saveTunnel = async (): Promise<void> => {
    const value = domain.value.trim();
    await invoke("tunnel_save", { provider, credential: token.value, publicUrl: value && !value.includes("://") ? `https://${value}` : value });
    token.value = "";
    saved = await invoke<TunnelConfig>("tunnel_config");
  };
  page.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach(button => {
    const name = button.dataset.tab!;
    button.id = `remote-tab-${name}`; button.setAttribute("aria-controls", `remote-panel-${name}`);
    const panel = page.querySelector<HTMLElement>(`[data-panel="${name}"]`)!;
    panel.id = `remote-panel-${name}`; panel.setAttribute("aria-labelledby", button.id);
    button.onclick = () => selectTab(name);
  });
  page.querySelector<HTMLElement>(".rs-tabs")!.onkeydown = event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    const tabs = [...page.querySelectorAll<HTMLButtonElement>("[data-tab]")];
    const index = tabs.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
    event.preventDefault(); selectTab(tabs[next].dataset.tab!, true);
  };
  find<HTMLInputElement>("enabled").onchange = () => void operation(() => controller.status ? controller.stop() : start());
  find("start").onclick = () => void operation(start);
  find<HTMLInputElement>("share").onchange = () => { const share = find<HTMLInputElement>("share").checked; void operation(() => controller.setDefaults({ ...controller.defaults, share })); };
  find<HTMLInputElement>("input").onchange = () => { const input = find<HTMLInputElement>("input").checked; void operation(() => controller.setDefaults({ ...controller.defaults, input })); };
  page.querySelectorAll<HTMLButtonElement>("[data-address]").forEach(button => { button.onclick = () => { addressKind = button.dataset.address!; render(); }; });
  address.onchange = () => { origin = address.value; void generateQr(); };
  find("refresh-qr").onclick = () => void generateQr();
  find("copy").onclick = () => { if (pairUrl) void writeText(pairUrl).then(() => { if (!disposed) find("result").textContent = "配对链接已复制。"; }).catch(report); };
  find("go-public").onclick = () => selectTab("public");
  find("manage").onclick = () => { find("devices").hidden = !find("devices").hidden; };
  find("revoke").onclick = () => void operation(async () => { await invoke("remote_revoke_clients"); find("devices").hidden = true; await generateQr(); });
  page.querySelectorAll<HTMLButtonElement>("[data-provider]").forEach(button => { button.onclick = () => { provider = button.dataset.provider as typeof provider; token.value = ""; render(); }; });
  find("save").onclick = () => void operation(saveTunnel);
  find("cleanup").onclick = () => void operation(async () => {
    const count = await invoke<number>("tunnel_cleanup");
    find("result").textContent = count
      ? `已清理 ${count} 个遗留进程，ngrok 云端状态同步可能稍有延迟。`
      : "未发现本版本的遗留连接；其他设备或版本的会话请在 ngrok 控制台管理。";
  });
  find("connect").onclick = () => void operation(async () => {
    if (tunnel.running) { await invoke("tunnel_stop"); return; }
    await saveTunnel();
    find("result").textContent = "正在下载并校验官方组件，首次连接可能需要一些时间…";
    await invoke("tunnel_install", { provider });
    if (!controller.status) throw new Error("远程访问已关闭，请重新开启后连接");
    await invoke("tunnel_start", { provider });
    find("result").textContent = "";
  });
  find("setup").onclick = () => void operation(async () => {
    const apiToken = find<HTMLInputElement>("api-token");
    try { domain.value = await invoke<string>("tunnel_cloudflare_setup", { apiToken: apiToken.value, hostname: find<HTMLInputElement>("hostname").value }); saved = await invoke<TunnelConfig>("tunnel_config"); find("result").textContent = "隧道已创建，点击连接即可。"; }
    finally { apiToken.value = ""; }
  });
  const unsubscribe = controller.subscribe(render);
  const timer = window.setInterval(() => void refresh(), 2000);
  void invoke<TunnelConfig>("tunnel_config").then(config => { if (disposed) return; saved = config; domain.value = config.cloudflareUrl; render(); }).catch(report);
  void refresh();
  render();
  return { page, dispose: () => { disposed = true; clearQr(); token.value = ""; find<HTMLInputElement>("api-token").value = ""; unsubscribe(); window.clearInterval(timer); } };
}

