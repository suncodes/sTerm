// 由 sTerm 在文件前注入本次运行的 CONFIG；只使用当前 Claude 安装版公开 Mod API。
let sessionId = "";
let turnId = "";
let streamId = "";
let sequence = 0;
let running = false;
let sessionActive = false;
let polling = false;
let flushing = false;
let queue = [];
let approvals = new Map();
let historyBusy = false;
let model = "";
let headers;
let timersStarted = false;
let liveText = "";
const resolveApproval = (id, request, nativeFinished = false) => {
  request.resolve(null); approvals.delete(id); emit({ type: "resolved", id });
  if (request.commandId) emit({ type: "result", id: request.commandId, ok: nativeFinished,
    message: nativeFinished ? "审批请求已由原生 Agent 结束" : "审批结果尚未确认，请核对原生终端" });
};
const toolCalls = new Map();
const emit = event => {
  if (queue.length < 512) queue.push(event); else needsSnapshot = true;
};
const clipped = value => String(value || "").slice(0, 48000);

async function flush($) {
  if (flushing || !sessionId || !queue.length) return;
  flushing = true;
  const batch = queue.splice(0, 128);
  try {
    const response = await $.http.fetch(CONFIG.url + "/event", {
      method: "POST", headers, body: JSON.stringify({ sessionId, events: batch }), timeoutMs: 1500,
    });
    if (response.status >= 400) throw new Error("bridge " + response.status);
  } catch {
    // 断桥不能挂住原生输出。重连成功后重新拉取原生历史以补齐。
    needsSnapshot = true;
  } finally { flushing = false; }
}

let needsSnapshot = true;
async function history($) {
  if (historyBusy) return;
  historyBusy = true;
  try {
    const messages = await $.session.messages();
    const items = [];
    for (let index = 0; index < messages.length; index++) {
      const message = messages[index];
      if (message.text) items.push({ id: "history-" + index, kind: message.role, text: clipped(message.text), status: "completed", title: "", turnId: "" });
      for (const tool of message.toolUses || []) {
        items.push({ id: "tool-" + (tool.tool_use_id || index + "-" + items.length), kind: "tool",
          title: tool.name || tool.tool || "工具", text: clipped(JSON.stringify(tool, null, 2)), status: tool.isError ? "error" : tool.result === undefined && tool.text === undefined ? "running" : "completed", turnId: "" });
      }
    }
    // 总量受本地 Hub 上限约束，避免一次传输完整超长历史。
    let size = 0;
    const bounded = [];
    for (const item of items.reverse()) { size += JSON.stringify(item).length; if (size > 200000) break; bounded.unshift(item); }
    emit({ type: "items", items: bounded });
    if (running && streamId) emit({ type: "upsert", item: { id: streamId, kind: "assistant", text: liveText, title: "", status: "running", turnId } });
    needsSnapshot = false;
  } catch (error) { emit({ type: "error", message: String(error) }); }
  finally { historyBusy = false; }
}
async function describe($) {
  const config = await $.config.list();
  const entry = config.find(item => item.key === "model");
  model = await $.session.model();
  emit({ type: "ready", sessionId, model, models: [...new Set([model, ...(entry?.options || [])])],
    capabilities: { send: true, stop: true, models: Boolean(entry) && !entry.isLocked, plan: true, policy: false, approvals: true } });
  await history($);
  emit({ type: "state", state: running ? "running" : "idle", turnId });
}
async function execute($, command) {
  try {
    if (command.sessionId !== sessionId) throw new Error("会话已切换");
    const claimed = await $.http.fetch(CONFIG.url + "/claim", { method: "POST", headers, body: JSON.stringify(command), timeoutMs: 1500 });
    if (claimed.status >= 400) throw new Error("操作已撤销或失效");
    switch (command.action) {
      case "sendText":
        const submitted = await $.prompt.submit({ text: command.text, asUser: true, wait: false });
        if (submitted.drop) throw new Error(submitted.drop);
        break;
      case "stopTurn":
        if (!turnId || turnId !== command.turnId) throw new Error("回合已结束");
        await $.turn.abort({ turnId }); break;
      case "setModel":
        if (running) throw new Error("请等待当前回合完成");
        const changed = await $.config.set({ key: "model", value: command.model });
        if (changed.deny) throw new Error(changed.deny);
        model = await $.session.model();
        emit({ type: "settings", model, policy: null }); break;
      case "enterPlan":
        if (running) throw new Error("请等待当前回合完成");
        await $.command.run({ command: "plan" }); break;
      case "respondApproval": {
        const request = approvals.get(command.requestId);
        if (!request || request.answered) throw new Error("审批已处理或失效");
        request.answered = true; request.commandId = command.id;
        request.resolve({ decision: { behavior: command.allow ? "allow" : "deny", ...(!command.allow ? { message: "用户在 sTerm 网页拒绝" } : {}) } });
        return; // 回执由工具/回合结束确认；唤醒 hook 还不表示原生决定已经落地。
      }
      default: throw new Error("不支持的操作");
    }
    emit({ type: "result", id: command.id, ok: true, message: null });
  } catch (error) { emit({ type: "result", id: command.id, ok: false, message: String(error) }); }
  void flush($);
}
async function poll($) {
  if (polling || !sessionId || !sessionActive) return;
  polling = true;
  try {
    const response = await $.http.fetch(CONFIG.url + "/poll", { headers, timeoutMs: 1500 });
    if (response.status >= 400) throw new Error("bridge " + response.status);
    const batch = JSON.parse(response.text);
    if (needsSnapshot) { queue = []; await describe($); }
    for (const command of batch.commands || []) await execute($, command);
  } catch {
    needsSnapshot = true;
    for (const [id, request] of approvals) resolveApproval(id, request);
  } finally { polling = false; }
}

export function register(on) {
  on("session.start", async ($, event, next) => {
    sessionId = await $.session.id(); sessionActive = true;
    const token = await $.env.get("__STERM_TOKEN_ENV__");
    if (!token) return next(event);
    headers = { "content-type": "application/json", authorization: "Bearer " + token };
    queue = []; running = false; turnId = ""; streamId = ""; liveText = "";
    try { await describe($); } catch { needsSnapshot = true; }
    if (!timersStarted) {
      timersStarted = true;
      $.clock.every(100, () => { void flush($); });
      $.clock.every(200, () => { void poll($); });
    }
    return next(event);
  });
  on("session.end", async ($, event, next) => {
    sessionActive = false; running = false; turnId = "";
    for (const [id, request] of approvals) resolveApproval(id, request);
    toolCalls.clear(); needsSnapshot = true;
    emit({ type: "state", state: "offline", turnId: "" });
    void flush($);
    return next(event);
  });
  on("turn.start", async ($, event, next) => {
    if (event.agentId === undefined) {
      const nativeId = await $.session.id();
      if (nativeId !== sessionId) { sessionId = nativeId; queue = []; needsSnapshot = true; }
      running = true; turnId = event.turnId; streamId = "live-" + (++sequence); liveText = "";
      emit({ type: "state", state: "running", turnId });
      emit({ type: "upsert", item: { id: streamId, kind: "assistant", text: "", title: "", status: "running", turnId } });
    }
    return next(event);
  });
  on("turn.step", async function* ($, event, next) {
    const stream = next(event);
    for await (const chunk of stream) {
      if (chunk.kind === "text" && event.agentId === undefined) { liveText = clipped(liveText + chunk.text); emit({ type: "delta", id: streamId, delta: chunk.text }); }
      yield chunk;
    }
    return await stream.result;
  });
  on("turn.complete", async ($, event, next) => {
    if (event.agentId === undefined) {
      running = false; turnId = "";
      for (const [id, request] of approvals) resolveApproval(id, request, true);
      toolCalls.clear();
      await history($);
      emit({ type: "state", state: "idle", turnId: "" });
    }
    return next(event);
  });
  on("classic.UserPromptSubmit", async ($, event, next) => {
    emit({ type: "settings", model: null, policy: event.permission_mode || "" });
    return next(event);
  });
  on("classic.PostModelSwitch", async ($, event, next) => {
    model = event.to_model; emit({ type: "settings", model, policy: null }); return next(event);
  });
  on("tool.call", async ($, event, next) => {
    const id = event.tool_use_id || "call-" + (++sequence);
    toolCalls.set(id, { tool: event.tool, input: event });
    emit({ type: "upsert", item: { id, kind: "tool", title: event.tool, text: clipped(JSON.stringify(event, null, 2)), status: "running", turnId } });
    try {
      const result = await next(event);
      emit({ type: "upsert", item: { id, kind: "tool", title: event.tool,
        text: clipped(JSON.stringify({ input: event, result }, null, 2)), status: result.isError ? "error" : "completed", turnId } });
      // 原生终端先处理审批时，工具已执行说明该调用对应的待办已失效。
      for (const [requestId, request] of approvals) {
        if (request.toolCallId === id) resolveApproval(requestId, request, true);
      }
      return result;
    } catch (error) {
      emit({ type: "upsert", item: { id, kind: "tool", title: event.tool, text: String(error), status: "error", turnId } });
      throw error;
    } finally {
      for (const [requestId, request] of approvals) if (request.toolCallId === id) resolveApproval(requestId, request, true);
      toolCalls.delete(id);
    }
  });
  on("classic.PreToolUse", async ($, event, next) => {
    for (const [id, call] of toolCalls) {
      if (id === event.tool_use_id) call.permissionInput = JSON.stringify(event.tool_input);
    }
    return next(event);
  });
  on("classic.PermissionRequest", async ($, event, next) => {
    const id = "permission-" + (++sequence);
    emit({ type: "approval", approval: { id, title: event.tool_name, detail: clipped(JSON.stringify(event.tool_input, null, 2)), turnId } });
    let settle;
    const answer = new Promise(resolve => { settle = resolve; });
    const toolCallId = [...toolCalls].find(([, call]) => !call.permissionAssigned && call.tool === event.tool_name && call.permissionInput === JSON.stringify(event.tool_input))?.[0];
    if (toolCallId) toolCalls.get(toolCallId).permissionAssigned = true;
    approvals.set(id, { resolve: settle, toolCallId, answered: false });
    // 原生审批 UI 同时存在。桥接不可达时不回答，交回原生审批。
    const deadline = $.clock.after(120000, () => { const request = approvals.get(id); if (request) resolveApproval(id, request); });
    const result = await answer;
    deadline.cancel();
    if (result) return result;
    approvals.delete(id);
    return next(event);
  });
}
