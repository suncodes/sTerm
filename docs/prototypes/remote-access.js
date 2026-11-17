const query = (selector) => document.querySelector(selector);
const all = (selector) => [...document.querySelectorAll(selector)];
const state = {
  enabled: true,
  defaultShare: true,
  defaultInput: false,
  connected: true,
  controllingId: null,
  activeId: 1,
  address: 'lan',
  provider: 'cloudflare',
  tunnelConnected: false,
  ctrl: false,
  fontSize: 12,
  fit: false,
  sheet: null,
  sessions: [
    { id: 1, name: 'sTerm · Codex', path: 'D:\\workspace\\sTerm', agent: 'Codex · 运行中', policy: 'inherit', dot: '#d1ae64', draft: '', output: [] },
    { id: 2, name: 'api-server · Claude', path: 'D:\\workspace\\api-server', agent: 'Claude · 等待输入', policy: 'inherit', dot: '#79bc8a', draft: '', output: [] },
    { id: 3, name: 'PowerShell', path: 'C:\\Users\\dev', agent: 'PowerShell 7', policy: 'inherit', dot: '#9797a3', draft: '', output: [] },
    { id: 4, name: 'notes · 私人', path: 'D:\\workspace\\notes', agent: 'PowerShell 7', policy: 'off', dot: '#9797a3', draft: '', output: [] },
  ],
};
const policies = [['inherit', '跟随默认'], ['off', '不分享'], ['readonly', '只读'], ['control', '允许接管']];
let toastTimer;
let sheetTrigger;

function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

function icon(name) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  node.setAttribute('class', 'icon');
  node.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  use.setAttribute('href', '#i-' + name);
  node.append(use);
  return node;
}

function toast(message) {
  clearTimeout(toastTimer);
  query('#prototype-toast').textContent = message;
  query('#prototype-toast').hidden = false;
  toastTimer = setTimeout(() => { query('#prototype-toast').hidden = true; }, 2600);
}

function permission(session) {
  if (session.policy === 'off') return { share: false, input: false };
  if (session.policy === 'readonly') return { share: true, input: false };
  if (session.policy === 'control') return { share: true, input: true };
  return { share: state.defaultShare, input: state.defaultShare && state.defaultInput };
}

function visibleSessions() {
  return state.sessions.filter((session) => permission(session).share);
}

function activeSession() {
  return state.sessions.find((session) => session.id === state.activeId);
}

function saveDraft() {
  const session = activeSession();
  if (session) session.draft = query('#client-draft').value;
}

function isControlling() {
  const session = activeSession();
  return Boolean(session && state.enabled && state.connected && permission(session).input && state.controllingId === session.id);
}

function reconcilePermissions() {
  const controller = state.sessions.find((session) => session.id === state.controllingId);
  if (!controller || !permission(controller).input || !state.enabled || !state.connected) state.controllingId = null;
  const visible = visibleSessions();
  if (!visible.some((session) => session.id === state.activeId)) {
    saveDraft();
    state.activeId = visible[0]?.id ?? null;
    state.ctrl = false;
  }
}

function renderPermissionSessions() {
  const list = query('#permission-sessions');
  list.replaceChildren();
  for (const session of state.sessions) {
    const row = element('div', 'permission-session');
    const dot = element('span', 'status-dot');
    dot.style.background = session.dot;
    const main = element('span', 'permission-session-main');
    const name = element('span', 'permission-session-name', session.name);
    name.title = session.path;
    const current = permission(session);
    const effective = !current.share ? '未分享' : current.input ? '允许接管' : '只读';
    const detail = element('span', 'permission-session-detail', session.policy === 'inherit' ? '跟随默认 · ' + effective : '单独设置 · ' + effective);
    main.append(name, detail);
    const select = element('select');
    select.setAttribute('aria-label', session.name + ' 的远程权限');
    select.dataset.session = String(session.id);
    for (const [value, label] of policies) select.add(new Option(label, value));
    select.value = session.policy;
    select.onchange = () => {
      saveDraft();
      session.policy = select.value;
      update();
      if (state.sheet === 'sessions') openSheet('sessions', false);
    };
    row.append(dot, main, select);
    list.append(row);
  }
  query('#session-count').textContent = String(state.sessions.length);
  query('#shared-count').textContent = String(visibleSessions().length);
}

function renderConnection() {
  query('#connect-enabled').hidden = !state.enabled;
  query('#connect-disabled').hidden = state.enabled;
  query('#service-state').textContent = state.enabled ? '已开启' : '已关闭';
  query('#service-state').classList.toggle('off', !state.enabled);
  query('#nav-live-dot').hidden = !state.enabled;
  query('#desktop-remote-status').textContent = state.enabled ? '远程访问已开启' : '远程访问已关闭';
  query('#remote-enabled').checked = state.enabled;
  const unavailable = state.address === 'public' && !state.tunnelConnected;
  query('#pairing-content').hidden = unavailable;
  query('#public-unavailable').hidden = !unavailable;
  query('#address-hint').textContent = state.address === 'lan' ? '手机与电脑连接同一网络' : '通过 HTTPS 访问，无需在同一网络';
  query('#connection-address').textContent = state.address === 'lan' ? 'http://192.168.1.106:9876' : 'https://terminal.example.com';
  query('#connected-count').textContent = state.connected && state.enabled ? '1' : '0';
  all('[data-address]').forEach((button) => button.setAttribute('aria-pressed', String(button.dataset.address === state.address)));
}

function selectSession(sessionId) {
  saveDraft();
  state.activeId = sessionId;
  state.ctrl = false;
  renderClient();
}

function syncSessionNavigation() {
  const session = activeSession();
  const showTabs = getComputedStyle(query('#client-tabs')).display !== 'none';
  query('#terminal-scroll').setAttribute('role', showTabs && session ? 'tabpanel' : 'region');
  query('#terminal-scroll').setAttribute('aria-labelledby', showTabs && session ? 'client-tab-' + session.id : 'client-current-title');
}

function renderTabs() {
  const current = activeSession();
  query('#client-current-title').textContent = current?.name ?? '暂无分享的会话';
  query('#client-current-title').title = current?.name ?? '';
  query('#client-current-dot').hidden = !current;
  if (current) query('#client-current-dot').style.background = current.dot;
  const tabs = query('#client-tabs');
  const focusedId = document.activeElement?.dataset.clientSession;
  tabs.replaceChildren();
  for (const session of visibleSessions()) {
    const tab = element('button', 'client-tab');
    const dot = element('span', 'status-dot');
    dot.style.background = session.dot;
    tab.append(dot, element('span', 'tab-label', session.name));
    tab.title = session.name + '\n' + session.path;
    tab.dataset.clientSession = String(session.id);
    tab.id = 'client-tab-' + session.id;
    tab.setAttribute('role', 'tab');
    tab.setAttribute('aria-selected', String(session.id === state.activeId));
    tab.setAttribute('aria-controls', 'terminal-scroll');
    tab.tabIndex = session.id === state.activeId ? 0 : -1;
    tab.onclick = () => selectSession(session.id);
    tabs.append(tab);
  }
  if (focusedId) tabs.querySelector('[aria-selected=true]')?.focus({ preventScroll: true });
  const selected = tabs.querySelector('[aria-selected=true]');
  if (selected) {
    if (selected.offsetLeft < tabs.scrollLeft) tabs.scrollLeft = selected.offsetLeft;
    else if (selected.offsetLeft + selected.offsetWidth > tabs.scrollLeft + tabs.clientWidth) tabs.scrollLeft = selected.offsetLeft + selected.offsetWidth - tabs.clientWidth;
  }
}

function terminalLine(content, className = '') {
  return element('div', className, content || '\u00a0');
}

function renderTerminal() {
  const output = query('#terminal-output');
  const session = activeSession();
  output.replaceChildren();
  if (!session) {
    output.append(element('div', 'sheet-empty', '暂无分享的会话，请在电脑端设置。'));
    return;
  }
  if (session.id === 1) {
    output.append(
      terminalLine('╭────────────────────────────────╮', 'term-rule'),
      terminalLine('│  >_ OpenAI Codex               │'),
      terminalLine('│  model: gpt-5.2-codex          │', 'term-dim'),
      terminalLine('│  directory: D:\\workspace\\sTerm │', 'term-dim'),
      terminalLine('╰────────────────────────────────╯', 'term-rule'),
      terminalLine(''),
      terminalLine('› 继续调整远程访问界面', 'term-blue'),
      terminalLine(''),
      terminalLine('• 已读取现有设置与终端主题。', 'term-dim'),
      terminalLine('  保留双栏导航，统一字号与间距。'),
      terminalLine(''),
      terminalLine('  修改内容', 'term-dim'),
      terminalLine('  + 设置内的远程访问入口', 'term-green'),
      terminalLine('  + 新会话默认权限', 'term-green'),
      terminalLine('  + 手机会话列表与输入区', 'term-green'),
      terminalLine(''),
      terminalLine('  当前为界面原型，等待设计审查。', 'term-yellow'),
      terminalLine(''),
      terminalLine('› 下一步需要调整什么？'),
    );
  } else if (session.id === 2) {
    output.append(
      terminalLine(' Claude Code', 'term-yellow'),
      terminalLine(' D:\\workspace\\api-server', 'term-dim'),
      terminalLine(''),
      terminalLine('› 检查服务启动状态', 'term-blue'),
      terminalLine(''),
      terminalLine('● API server is running', 'term-green'),
      terminalLine('  Local: http://localhost:3000', 'term-dim'),
      terminalLine('  Health check: 200 OK', 'term-dim'),
      terminalLine(''),
      terminalLine('  已完成检查，等待下一条指令。'),
      terminalLine(''),
    );
  } else {
    output.append(terminalLine('PowerShell 7.5.0', 'term-dim'), terminalLine(''), terminalLine('PS ' + session.path + '> '));
  }
  for (const sent of session.output) output.append(terminalLine(sent, 'term-blue'));
  const cursorLine = terminalLine('');
  cursorLine.replaceChildren(element('span', 'term-cursor'));
  output.append(cursorLine);
}

function renderClient() {
  const session = activeSession();
  const online = state.connected && state.enabled;
  const allowed = Boolean(session && permission(session).input);
  const control = isControlling();
  renderTabs();
  query('#client-path').textContent = session?.path ?? '暂无会话';
  query('#client-path').title = session?.path ?? '';
  query('#client-connection').replaceChildren(element('span', 'status-dot ' + (online ? 'green' : 'gold')), document.createTextNode(online ? '已连接' : '已断线'));
  query('#client-connection').classList.toggle('disconnected', !online);
  query('#control-badge').textContent = control ? '控制中' : '只读';
  query('#control-badge').classList.toggle('controlling', control);
  query('#claim-control').hidden = !online || !allowed || control;
  query('#release-control').hidden = !control;
  query('#client-input').hidden = !control;
  query('#desktop-controlbar').hidden = !state.controllingId;
  query('#desktop-controller-session').textContent = state.sessions.find((item) => item.id === state.controllingId)?.name ?? '';
  query('#readonly-hint').hidden = control;
  query('#client-draft').disabled = !control;
  query('#offline-banner').hidden = online;
  query('#reconnect').disabled = !state.enabled;
  query('#readonly-description').textContent = !state.enabled ? '电脑端已关闭远程访问' : !online ? '重新连接后可继续操作，草稿已保留' : !session ? '在电脑端开启默认分享或单独分享会话' : allowed ? '点击「接管」后可输入，电脑可随时收回' : '电脑端尚未允许此会话输入';
  query('#remote-app').dataset.controlling = String(control);
  query('#client-draft').value = session?.draft ?? '';
  query('#demo-state').value = !online ? 'offline' : control ? 'control' : allowed ? 'available' : 'readonly';
  query('#ctrl-key').setAttribute('aria-pressed', String(state.ctrl));
  renderTerminal();
  refreshDraft();
  syncSessionNavigation();
  scrollBottom();
}

function update() {
  reconcilePermissions();
  query('#default-share').checked = state.defaultShare;
  query('#default-input').checked = state.defaultInput;
  query('#default-input').disabled = !state.defaultShare;
  renderPermissionSessions();
  renderConnection();
  renderClient();
}

function setPage(page, focus = false) {
  for (const button of all('[data-page]')) {
    const selected = button.dataset.page === page;
    button.setAttribute('aria-selected', String(selected));
    button.tabIndex = selected ? 0 : -1;
    query('#panel-' + button.dataset.page).hidden = !selected;
    if (selected && focus) button.focus();
  }
}

function scrollBottom() {
  const terminal = query('#terminal-scroll');
  terminal.scrollTop = terminal.scrollHeight;
  query('#terminal-jump').hidden = true;
}

function refreshDraft() {
  const draft = query('#client-draft');
  draft.style.height = 'auto';
  draft.style.height = Math.min(128, Math.max(56, draft.scrollHeight)) + 'px';
  query('#send-input').disabled = !isControlling() || !draft.value.trim();
}

function sendInput() {
  if (!isControlling() || !query('#client-draft').value.trim()) return;
  const session = activeSession();
  const suffix = query('#append-enter').checked ? ' ↵' : '';
  session.output.push('› ' + query('#client-draft').value + suffix);
  session.draft = '';
  query('#client-draft').value = '';
  renderTerminal();
  refreshDraft();
  scrollBottom();
  toast('演示：已发送至当前会话');
}

function sendKey(key) {
  if (!isControlling()) return;
  const label = (state.ctrl ? 'Ctrl+' : '') + key;
  state.ctrl = false;
  query('#ctrl-key').setAttribute('aria-pressed', 'false');
  toast('演示快捷键：' + label);
}

function closeSheet() {
  state.sheet = null;
  query('#client-sheet-backdrop').hidden = true;
  query('#all-sessions').setAttribute('aria-expanded', 'false');
  sheetTrigger?.focus({ preventScroll: true });
}

function openSheet(kind, captureFocus = true) {
  if (captureFocus) sheetTrigger = document.activeElement;
  state.sheet = kind;
  query('#client-sheet-backdrop').hidden = false;
  query('#client-sheet-backdrop').dataset.sheet = kind;
  query('#all-sessions').setAttribute('aria-expanded', String(kind === 'sessions'));
  const content = query('#sheet-content');
  content.replaceChildren();
  query('#sheet-title').textContent = { sessions: '所有会话', keys: '更多快捷键', display: '显示与连接' }[kind];
  if (kind === 'sessions') {
    for (const session of visibleSessions()) {
      const button = element('button', 'sheet-session');
      button.dataset.sessionId = String(session.id);
      const dot = element('span', 'status-dot');
      dot.style.background = session.dot;
      const description = element('span');
      description.append(element('strong', '', session.name), element('small', '', session.agent + ' · ' + (permission(session).input ? '可接管' : '只读')));
      button.append(dot, description);
      if (session.id === state.activeId) { button.append(icon('check')); button.setAttribute('aria-current', 'true'); }
      button.onclick = () => { selectSession(session.id); closeSheet(); };
      content.append(button);
    }
    if (!visibleSessions().length) content.append(element('div', 'sheet-empty', '暂无分享的会话'));
  } else if (kind === 'keys') {
    const grid = element('div', 'keys-grid');
    for (const key of ['Ctrl+C', 'Ctrl+D', 'Ctrl+L', 'Shift+Tab', 'Home', 'End', 'Page Up', 'Page Down', 'Enter']) {
      const button = element('button', '', key);
      button.disabled = !isControlling();
      button.onclick = () => { state.ctrl = false; sendKey(key); closeSheet(); };
      grid.append(button);
    }
    content.append(grid);
  } else {
    const row = element('div', 'sheet-row');
    row.append(element('span', '', '终端字号'));
    const stepper = element('div', 'font-stepper');
    const output = element('output', '', state.fontSize + ' px');
    for (const [label, delta] of [['−', -1], ['+', 1]]) {
      const button = element('button', '', label);
      button.setAttribute('aria-label', delta < 0 ? '减小终端字号' : '增大终端字号');
      button.onclick = () => { state.fontSize = Math.max(10, Math.min(20, state.fontSize + delta)); query('#remote-app').style.setProperty('--terminal-font', state.fontSize + 'px'); output.textContent = state.fontSize + ' px'; };
      if (delta < 0) stepper.append(button, output);
      else stepper.append(button);
    }
    row.append(stepper);
    const fit = element('label', 'sheet-row');
    fit.append(element('span', '', '适配屏幕宽度'));
    const toggle = element('input', 'settings-switch');
    toggle.type = 'checkbox';
    toggle.checked = state.fit;
    toggle.onchange = () => { state.fit = toggle.checked; query('#terminal-scroll').classList.toggle('fit-text', state.fit); };
    fit.append(toggle);
    content.append(row, fit);
    for (const [title, glyph, action] of [['所有会话', 'list', () => openSheet('sessions', false)], ['重新连接', 'refresh', () => { saveDraft(); state.connected = true; state.controllingId = null; closeSheet(); update(); toast('演示：已重新连接，控制权已释放'); }]]) {
      const button = element('button', 'sheet-menu-button', title);
      button.prepend(icon(glyph));
      button.onclick = action;
      content.append(button);
    }
  }
  if (captureFocus) query('#close-sheet').focus({ preventScroll: true });
}

function setProvider(provider) {
  if (provider === state.provider) return;
  state.provider = provider;
  state.tunnelConnected = false;
  all('[data-provider]').forEach((button) => button.setAttribute('aria-pressed', String(button.dataset.provider === provider)));
  const cloudflare = provider === 'cloudflare';
  query('#provider-description').textContent = cloudflare ? '适合日常使用，通过自己的域名访问。' : '适合快速测试，使用 ngrok 账号的开发域名。';
  query('#token-label').textContent = cloudflare ? 'Tunnel Token' : 'Authtoken';
  query('#tunnel-token').placeholder = cloudflare ? '粘贴 Cloudflare 的隧道令牌' : '粘贴 ngrok Authtoken';
  query('#tunnel-token').value = '';
  query('#tunnel-domain').value = '';
  query('#tunnel-domain').placeholder = cloudflare ? 'terminal.example.com' : 'your-name.ngrok-free.app';
  query('#provider-help').replaceChildren();
  query('#provider-help').append(element('summary', '', cloudflare ? '首次使用 Cloudflare' : '首次使用 ngrok'));
  const steps = element('ol');
  const instructions = cloudflare ? ['将自己的域名接入 Cloudflare。', '在 Zero Trust 中创建 Tunnel，复制 Token。', '添加公网主机名，指向 http://127.0.0.1:9876。'] : ['注册 ngrok 账号。', '在控制台复制 Authtoken 和开发域名。', '粘贴至当前页面，桌面端负责启动隧道。'];
  for (const instruction of instructions) steps.append(element('li', '', instruction));
  query('#provider-help').append(steps);
  renderTunnel();
}

function renderTunnel() {
  const name = state.provider === 'cloudflare' ? 'Cloudflare' : 'ngrok';
  query('#connect-tunnel').textContent = state.tunnelConnected ? '断开连接' : '连接 ' + name;
  query('#tunnel-state').textContent = state.tunnelConnected ? '已连接' : '未连接';
  query('#tunnel-state').classList.toggle('off', !state.tunnelConnected);
  query('#tunnel-domain-state').textContent = state.tunnelConnected ? '演示：terminal.example.com' : '未连接时仍可使用局域网二维码';
  renderConnection();
}

all('[data-view]').filter((button) => button.tagName === 'BUTTON').forEach((button) => {
  button.onclick = () => {
    document.body.dataset.view = button.dataset.view;
    all('.view-switch button').forEach((item) => item.setAttribute('aria-pressed', String(item === button)));
    query('#remote-caption').textContent = button.dataset.view === 'browser' ? '浏览器 / 当前会话' : '手机端 / 当前会话';
    query('#remote-caption-size').textContent = button.dataset.view === 'browser' ? '自适应宽度' : '390px';
    syncSessionNavigation();
  };
});

all('[data-settings-page]').forEach((button) => {
  button.onclick = () => {
    all('[data-settings-page]').forEach((item) => { item.classList.toggle('active', item === button); item.removeAttribute('aria-current'); });
    button.setAttribute('aria-current', 'page');
    const remote = button.dataset.settingsPage === 'remote';
    query('#remote-settings').hidden = !remote;
    query('#other-settings').hidden = remote;
    if (!remote) {
      const panel = query('#other-settings');
      panel.replaceChildren(element('h2', 'page-title', button.textContent.trim()));
      if (button.dataset.settingsPage === 'appearance') {
        panel.className = 'appearance-example';
        for (const [label, size] of [['标签栏字号', '12 px'], ['侧边栏字号', '12 px'], ['终端字号', '13 px']]) {
          const row = element('div', 'settings-row');
          row.append(element('span', 'settings-label', label), element('span', 'settings-value', size));
          panel.append(row);
        }
        panel.append(element('p', 'settings-hint', '原有设置页样式参考。新增「远程访问」沿用同一套控件与主题。'));
      } else panel.append(element('p', 'settings-hint', '此页仅演示原有设置分类。请选择「远程访问」审查本次调整。'));
    }
  };
});

query('#open-settings').onclick = () => { query('#settings-overlay').hidden = false; query('#close-settings').focus(); };
query('#close-settings').onclick = () => { query('#settings-overlay').hidden = true; query('#open-settings').focus(); };
all('[data-page]').forEach((button) => { button.onclick = () => setPage(button.dataset.page); });
query('.settings-subtabs').onkeydown = (event) => {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  const tabs = all('[data-page]');
  const index = tabs.indexOf(document.activeElement);
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
  setPage(tabs[next].dataset.page, true);
};

query('#remote-enabled').onchange = () => { saveDraft(); state.enabled = query('#remote-enabled').checked; update(); };
query('#enable-service').onclick = () => { state.enabled = true; state.connected = true; update(); };
query('#default-share').onchange = () => { saveDraft(); state.defaultShare = query('#default-share').checked; update(); };
query('#default-input').onchange = () => { saveDraft(); state.defaultInput = query('#default-input').checked; update(); };
query('#new-session').onclick = () => {
  const id = Math.max(...state.sessions.map((session) => session.id)) + 1;
  state.sessions.push({ id, name: '新会话 ' + (id - 4), path: 'D:\\workspace\\new-session', agent: 'PowerShell 7', policy: 'inherit', dot: '#9797a3', draft: '', output: [] });
  saveDraft();
  update();
  toast('新会话已跟随默认：' + (!state.defaultShare ? '不分享' : state.defaultInput ? '允许接管' : '只读'));
};
all('[data-address]').forEach((button) => { button.onclick = () => { state.address = button.dataset.address; renderConnection(); }; });
query('#go-public').onclick = () => setPage('public');
query('#refresh-qr').onclick = () => { refreshExpiry(); toast('演示：二维码已刷新；现有设备保持连接'); };
query('#copy-address').onclick = async () => {
  const demoLink = 'https://example.invalid/sterm-demo#prototype-only';
  try { await navigator.clipboard.writeText(demoLink); toast('已复制演示链接；实际扫码不可连接'); }
  catch { toast('演示配对链接：https://example.invalid/sterm-demo'); }
};
query('#show-devices').onclick = () => { query('#connected-devices').hidden = !query('#connected-devices').hidden; };
query('#revoke-device').onclick = () => { saveDraft(); state.connected = false; update(); query('#connected-devices').hidden = true; toast('演示设备已撤销访问'); };
all('[data-provider]').forEach((button) => { button.onclick = () => setProvider(button.dataset.provider); });
query('#connect-tunnel').onclick = () => { state.tunnelConnected = !state.tunnelConnected; renderTunnel(); toast(state.tunnelConnected ? '演示公网已连接，未调用任何服务' : '演示公网已断开，局域网仍可用'); };

query('#demo-state').onchange = () => {
  saveDraft();
  const mode = query('#demo-state').value;
  state.enabled = true;
  state.connected = mode !== 'offline';
  state.controllingId = null;
  let session = activeSession();
  if (!session) { session = state.sessions[0]; state.activeId = session.id; }
  if (mode !== 'offline') {
    state.defaultShare = true;
    state.defaultInput = mode !== 'readonly';
    session.policy = 'inherit';
    if (mode === 'control') state.controllingId = session.id;
  }
  state.ctrl = false;
  update();
};
query('#claim-control').onclick = () => { state.controllingId = state.activeId; renderClient(); toast('演示：已接管当前会话，电脑可随时收回'); };
query('#release-control').onclick = () => { saveDraft(); state.controllingId = null; state.ctrl = false; renderClient(); };
query('#desktop-reclaim').onclick = () => { saveDraft(); state.controllingId = null; state.ctrl = false; renderClient(); toast('演示：电脑已收回控制权'); };
query('#reconnect').onclick = () => { state.connected = true; update(); toast('演示：已重新连接，需要重新接管后输入'); };
query('#all-sessions').onclick = () => openSheet('sessions');
query('#client-menu').onclick = () => openSheet('display');
query('#more-keys').onclick = () => openSheet('keys');
query('#close-sheet').onclick = closeSheet;
query('#client-sheet-backdrop').onclick = (event) => { if (event.target === query('#client-sheet-backdrop')) closeSheet(); };
query('#client-sheet-backdrop').onkeydown = (event) => {
  if (event.key !== 'Tab') return;
  const focusable = all('#client-sheet-backdrop button, #client-sheet-backdrop input').filter((node) => !node.disabled);
  const first = focusable[0];
  const last = focusable.at(-1);
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
};
query('#client-tabs').onkeydown = (event) => {
  if (getComputedStyle(query('#client-tabs')).display === 'none') return;
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  const sessions = visibleSessions();
  const index = sessions.findIndex((session) => session.id === state.activeId);
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? sessions.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + sessions.length) % sessions.length;
  if (sessions[next]) selectSession(sessions[next].id);
};
query('#ctrl-key').onclick = () => { state.ctrl = !state.ctrl; query('#ctrl-key').setAttribute('aria-pressed', String(state.ctrl)); };
all('[data-key]').forEach((button) => { button.onclick = () => sendKey(button.dataset.key); });
query('#client-draft').oninput = () => { saveDraft(); refreshDraft(); };
query('#client-draft').onkeydown = (event) => {
  if (event.isComposing) return;
  if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); sendInput(); }
};
query('#send-input').onclick = sendInput;
query('#terminal-scroll').onscroll = () => {
  const terminal = query('#terminal-scroll');
  query('#terminal-jump').hidden = terminal.scrollHeight - terminal.scrollTop - terminal.clientHeight < 64;
};
query('#scroll-bottom').onclick = scrollBottom;
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  if (state.sheet) closeSheet();
  else if (!query('#settings-overlay').hidden && document.body.dataset.view !== 'mobile' && document.body.dataset.view !== 'browser') { query('#settings-overlay').hidden = true; query('#open-settings').focus(); }
});
window.addEventListener('resize', syncSessionNavigation);

function refreshExpiry() {
  query('#qr-expiry span').textContent = new Date(Date.now() + 5 * 60 * 1000).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

const initialView = new URLSearchParams(location.search).get('view');
if (['desktop', 'mobile', 'browser', 'compare'].includes(initialView)) query('.view-switch button[data-view=' + initialView + ']').click();
else if (window.innerWidth < 760) query('.view-switch button[data-view=mobile]').click();
renderTunnel();
refreshExpiry();
update();
