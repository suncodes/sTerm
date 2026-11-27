// 由 sTerm 注入本次运行的 CONFIG；只使用当前 Claude 安装版公开 Mod API。
let sessionId = "", sequence = 0, model = "", headers;
let sessionActive = false, timersStarted = false, polling = false, flushing = false, needsSnapshot = true;
let queue = [];
let currentActivity = {};
const loops = new Map(), requests = new Map(), toolCalls = new Map();
const clipped = value => String(value || "").slice(0, 48000);
const emit = event => { if(event.type==="activity")currentActivity=event.value;if (queue.length < 512) queue.push(event); else needsSnapshot = true; };
const taskInfo = new Map();
const transcripts = new Map(), turnRecords = new Map(), backgroundTasks = new Map();
const nativeNotifications = new Set();
const historyBusy = new Set();
const boundedData = value => JSON.stringify(value ?? {}).length > 60000 ? {truncated:true} : value;
const toolInput = event => event.tool_input ?? Object.fromEntries(Object.entries(event).filter(([key])=>!["tool","tool_use_id","agentId","requestMeta","consent"].includes(key)));
const timeline = agentId => { const key=agentId||""; if(!transcripts.has(key))transcripts.set(key,[]); return transcripts.get(key); };
function putItem(item) {
  item={...item,data:boundedData(item.data)};
  const rows=timeline(item.agentId),index=rows.findIndex(row=>row.id===item.id);
  if(index<0)rows.push(item);else rows[index]={...rows[index],...item,data:{...rows[index].data,...item.data}};
  let size=rows.reduce((total,row)=>total+JSON.stringify(row).length,0);
  while(rows.length>500||size>200000){size-=JSON.stringify(rows.shift()).length;}
  emit({type:"upsert",item});
}
function toolSummary(result) {
  if(typeof result==="string")return result;
  return result?.text || [result?.stdout,result?.stderr].filter(Boolean).join("\n") || result?.deny || (typeof result?.content==="string"?result.content:Array.isArray(result?.content)?result.content.filter(part=>part.type==="text").map(part=>part.text).join("\n"):"") || (result?.result?toolSummary(result.result):"");
}
function backgroundResult(tool,input,result,itemId,agentId,startedAtMs) {
  const body=result?.result??result;
  const taskId=body?.task_id||body?.backgroundTaskId||body?.shell_id||input?.task_id;
  if(!taskId||!["Bash","TaskOutput","BashOutput","KillShell","TaskStop","Monitor"].includes(tool))return;
  const previous=backgroundTasks.get(taskId);
  if(!previous&&!input?.run_in_background&&!body?.backgroundTaskId)return;
  const state=["completed","failed","stopped"].includes(body?.status)?body.status:["KillShell","TaskStop"].includes(tool)&&!result?.isError&&!result?.deny?"stopped":"running";
  const task={id:"shell:"+taskId,parentId:agentId||"",title:input?.description||input?.command||previous?.title||tool,kind:"shell",state,
    model:"",turnId:loopFor(agentId)?.turnId||"",message:clipped(toolSummary(result)),data:{...previous?.data,nativeTaskId:taskId,itemId:previous?.data?.itemId||itemId,
      startedAtMs:previous?.data?.startedAtMs||startedAtMs,source:"nativeToolResult",timeSource:"observed",backgroundConfirmed:true,canStop:false,...(state!=="running"?{completedAtMs:Date.now()}:{}),outputFile:body?.output_file||previous?.data?.outputFile}};
  backgroundTasks.set(taskId,task);emit({type:"task",task});
}
function taskNotification(text) {
  if(typeof text!=="string"||!text.trim().startsWith("<task-notification>"))return false;
  const field=name=>text.match(new RegExp("<"+name+">([\\s\\S]*?)</"+name+">"))?.[1]?.trim();
  const taskId=field("task-id"),status=field("status"),previous=backgroundTasks.get(taskId);
  if(!previous||!["completed","failed","stopped","killed"].includes(status))return false;
  if(previous.state===status)return true;
  const task={...previous,state:status,message:clipped(field("summary")),data:{...previous.data,completedAtMs:Date.now(),timeSource:"observed",outputFile:field("output-file")||previous.data.outputFile}};
  backgroundTasks.set(taskId,task);emit({type:"task",task});
  emit({type:"notice",value:{id:"task:"+taskId,agentId:previous.parentId,title:"后台命令已结束",message:task.message,severity:status==="completed"?"info":"warning"}});
  return true;
}
const loopFor = agentId => loops.get(agentId || "");
const main = () => loopFor("");
function resolveRequest(id, nativeFinished = false) {
  const request = requests.get(id);
  if (!request) return;
  requests.delete(id); emit({ type: "resolved", id });
  if (request.commandId) emit({ type: "result", id: request.commandId, ok: nativeFinished,
    message: nativeFinished ? "原生 Agent 已处理回答" : "回答结果尚未确认，请核对原生终端" });
}
async function flush($) {
  if (flushing || !sessionId || !queue.length) return;
  flushing = true;
  const batch = [];let length=0;
  while(queue.length&&batch.length<128){const size=JSON.stringify(queue[0]).length;if(batch.length&&length+size>240000)break;length+=size;batch.push(queue.shift());}
  try {
    const response = await $.http.fetch(CONFIG.url + "/event", {
      method: "POST", headers, body: JSON.stringify({ sessionId, events: batch }), timeoutMs: 1500,
    });
    if (response.status >= 400) throw new Error("bridge " + response.status);
  } catch { needsSnapshot = true; }
  finally { flushing = false; }
}
function taskEvent(info, state, turnId = "", message = "") {
  info = {...taskInfo.get(info.id),...info}; taskInfo.set(info.id, info);
  return { type: "task", task: { id: info.id, parentId: info.parentId || "", title: info.description || info.name || info.type || "子 Agent",
    model: info.model || "", state: state || info.status || "unknown", turnId, message: clipped(message||info.label),kind:"agent",data:{...info.data,startedAtMs:info.startTime,...(turnRecords.get(info.id+":"+(turnId||loops.get(info.id)?.turnId))||{}),cwd:info.cwd} } };
}
async function history($, agentId = "") {
  if (historyBusy.has(agentId)) return;
  historyBusy.add(agentId);
  try {
    if(transcripts.has(agentId)) {
      emit(agentId?{type:"taskItems",agentId,items:timeline(agentId)}:{type:"items",items:timeline(agentId)});
      if(!agentId)needsSnapshot=false;return;
    }
    let messages = await $.session.messages({as:"api",...(agentId?{agentId}:{})});
    if(!Array.isArray(messages))messages=[];
    const items = [], prefix = agentId ? agentId + ":" : "";
    for (let index = 0; index < messages.length; index++) {
      const message = messages[index];
      for(const [blockIndex,block] of (Array.isArray(message.content)?message.content:typeof message.content==="string"?[{type:"text",text:message.content}]:[]).entries()) {
        const base={id:prefix+"history-"+index+"-"+blockIndex,status:"completed",title:"",turnId:"",agentId,data:{type:block.type}};
        if(block.type==="text"||block.type==="thinking")items.push({...base,kind:block.type==="thinking"?"thinking":message.role,text:clipped(block.text||block.thinking)});
        else if(block.type==="tool_use")items.push({...base,id:prefix+block.id,kind:"tool",title:block.name,text:"",data:{type:"claudeTool",input:block.input}});
        else if(block.type==="tool_result") {const call=items.find(row=>row.id===prefix+block.tool_use_id);if(call){call.text=clipped(toolSummary(block));call.status=block.is_error?"error":"completed";call.data.result=boundedData(block);}}
        else if(["image","document","audio"].includes(block.type))items.push({...base,kind:"image",title:block.type==="image"?"图片附件":"媒体附件",text:"",data:{type:block.type,source:block.source}});
      }
      if (!message.content && message.text) items.push({ id: prefix + "history-" + index, kind: message.role, text: clipped(message.text), status: "completed", title: "", turnId: "", agentId });
      for (const tool of message.toolUses || []) {
        items.push({ id: prefix + "tool-" + (tool.tool_use_id || index + "-" + items.length), kind: "tool", agentId,
          title: tool.name || tool.tool || "工具", text: clipped(toolSummary(tool)),
          status: tool.isError ? "error" : tool.result === undefined && tool.text === undefined ? "running" : "completed", turnId: "",data:boundedData({type:"claudeTool",input:tool.input,result:tool.result}) });
      }
    }
    let size = 0; const bounded = [];
    for (const item of items.reverse()) { size += JSON.stringify(item).length; if (size > 200000) break; bounded.unshift(item); }
    transcripts.set(agentId,bounded);
    emit(agentId ? { type: "taskItems", agentId, items: bounded } : { type: "items", items: bounded });
    if (!agentId) needsSnapshot = false;
  } catch (error) { emit({ type: "error", message: clipped(error) }); }
  finally { historyBusy.delete(agentId); }
}
async function agents($) {
  try { for (const info of (await $.agent.list()).slice(0, 64)) emit(taskEvent(info)); }
  catch { /* 旧版未提供时，仍可显示 turn 事件观察到的子任务。 */ }
}
async function describe($) {
  const config = await $.config.list(), entry = config.find(item => item.key === "model");
  model = await $.session.model();
  emit({ type: "ready", sessionId, model, models: [...new Set([model, ...(entry?.options || [])])],
    capabilities: { send: true, stop: true, models: Boolean(entry) && !entry.isLocked, plan: true, policy: false, approvals: true, queue: false } });
  await history($);for(const agentId of transcripts.keys())if(agentId)await history($,agentId);await agents($);
  emit({ type: "activity", value: currentActivity });
  emit({ type: "state", state: main()?.running ? "running" : main()?.state||"idle", turnId: main()?.running?main().turnId:"" });
  for (const request of requests.values()) emit({ type: "interaction", interaction: request.interaction });
  for(const value of turnRecords.values())emit({type:"turn",value});
  for(const task of backgroundTasks.values())emit({type:"task",task});
}
async function execute($, command) {
  try {
    if (command.sessionId !== sessionId) throw new Error("会话已切换");
    const claimed = await $.http.fetch(CONFIG.url + "/claim", { method: "POST", headers, body: JSON.stringify(command), timeoutMs: 1500 });
    if (claimed.status >= 400) throw new Error("操作已撤销或失效");
    switch (command.action) {
      case "sendText": {
        const submitted = await $.prompt.submit({ text: command.text, asUser: true, wait: false });
        if (submitted.drop) throw new Error(submitted.drop); break;
      }
      case "stopTurn":
        if (!main()?.turnId || main().turnId !== command.turnId) throw new Error("回合已结束");
        await $.turn.abort({ turnId: command.turnId }); break;
      case "setModel": {
        if (main()?.running) throw new Error("请等待当前回合完成");
        const changed = await $.config.set({ key: "model", value: command.model });
        if (changed.deny) throw new Error(changed.deny);
        model = await $.session.model(); emit({ type: "settings", model, policy: null }); break;
      }
      case "enterPlan":
        if (main()?.running) throw new Error("请等待当前回合完成");
        await $.command.run({ command: "plan" }); break;
      default: throw new Error("不支持的操作");
    }
    emit({ type: "result", id: command.id, ok: true, message: null });
  } catch (error) { emit({ type: "result", id: command.id, ok: false, message: clipped(error) }); }
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
  } catch { needsSnapshot = true; }
  finally { polling = false; }
}
// 等待必须在真正的 $ API 调用内；普通 Promise/定时器会消耗 Mod hook 的 10 秒自身预算。
async function waitResponse($, next, request) {
  const { id } = request.interaction;
  requests.set(id, request);
  const aborted = () => resolveRequest(id, true);
  next.signal.addEventListener("abort", aborted, { once: true });
  try {
    // 与并发 flush 分开确认注册，避免 busy flush 提前返回后 wait 误判“已结束”。
    const registered = await $.http.fetch(CONFIG.url + "/event", {
      method: "POST", headers, body: JSON.stringify({ sessionId, events: [{ type: "interaction", interaction: request.interaction }] }), timeoutMs: 1500,
    });
    if (registered.status >= 400) return null;
    while (!next.signal.aborted && requests.has(id) && sessionActive) {
      const response = await $.http.fetch(CONFIG.url + "/wait/" + encodeURIComponent(id), { headers, timeoutMs: 25000 });
      if (response.status >= 400) return null;
      const batch = JSON.parse(response.text);
      if (batch.resolved) return null;
      if (batch.command) {
        if (!requests.has(id) || next.signal.aborted) return null;
        request.commandId = batch.command.id;
        return batch.command.response;
      }
    }
    return null;
  } catch { return null; }
  finally {
    next.signal.removeEventListener("abort", aborted);
    // Native signal/工具结束会同步清理网页卡片；断桥交回原生，不产生默认批准。
    if (!request.commandId) { resolveRequest(id); await flush($); }
  }
}
function sourceFor(event) {
  // 只有唯一原生 tool_use_id 或唯一输入匹配才认定来源，不能把并发审批分配给猜测的工具。
  if (event.tool_use_id && toolCalls.has(event.tool_use_id)) return [event.tool_use_id, toolCalls.get(event.tool_use_id)];
  const candidates = [...toolCalls].filter(([, call]) => !call.assigned && call.tool === event.tool_name && call.agentId === (event.agent_id || ""));
  const exact = candidates.filter(([,call])=>call.permissionInput===JSON.stringify(event.tool_input));
  if (exact.length === 1) return exact[0];
  return candidates.length === 1 ? candidates[0] : [null, null];
}
function permissionInteraction(event, id, call) {
  const input = event.tool_input || {}, agentId = event.agent_id || call?.agentId || "", turnId = loopFor(agentId)?.turnId || "";
  const base = { id, title: event.tool_name, detail: input.description||event.message||"", agentId, turnId, blocking: true };
  if (event.tool_name === "AskUserQuestion") return { ...base, detail: "", title: "Claude 需要你回答", kind: "question",
    data: { cancellable: true, questions: (input.questions || []).map((q, index) => ({ id: String(index), title: q.question, header: q.header,
      multiSelect: Boolean(q.multiSelect), other: true, options: q.options || [], preview: q.preview })) } };
  const decisions = [{ label: "批准一次", value: "allow" }, { label: "拒绝", value: "deny" }];
  // 会话规则只使用该原生请求提供的建议；计划确认不隐式扩大执行模式。
  const suggestions = (event.permission_suggestions || []).filter(p => p.destination === "session" && p.type === "addRules" && p.behavior === "allow");
  if (suggestions.length && event.tool_name !== "ExitPlanMode") decisions.splice(1, 0, { label: "本会话允许建议规则", value: "allowSession" });
  return { ...base, kind: event.tool_name === "ExitPlanMode" ? "plan" : "approval",
    title: event.tool_name === "ExitPlanMode" ? "确认执行计划" : event.tool_name, ...(event.tool_name==="ExitPlanMode"&&input.plan?{detail:clipped(input.plan)}:{}), data: { decisions, suggestions,context:{tool:event.tool_name,input,itemId:call?.itemId} } };
}
function permissionResult(interaction, event, answer) {
  if (interaction.kind === "question") {
    if (answer.cancel) return { decision: { behavior: "deny", message: "用户取消了本次提问" } };
    const answers = {};
    for (let i = 0; i < (event.tool_input.questions || []).length; i++) {
      const q = event.tool_input.questions[i], values = answer.answers?.[String(i)];
      if (!Array.isArray(values) || !values.length || !q.multiSelect && values.length !== 1) throw new Error("问题答案不完整");
      answers[q.question] = values.join(", ");
    }
    return { decision: { behavior: "allow", updatedInput: { ...event.tool_input, answers } } };
  }
  if (answer.decision === "deny") return { decision: { behavior: "deny", message: "用户在 sTerm 网页拒绝" } };
  if (answer.decision !== "allow" && answer.decision !== "allowSession") throw new Error("无效审批决定");
  return { decision: { behavior: "allow", ...(answer.decision === "allowSession" ? { updatedPermissions: interaction.data.suggestions } : {}) } };
}
export function register(on) {
  on("session.start", async ($, event, next) => {
    sessionId = await $.session.id(); sessionActive = true;
    const token = await $.env.get("__STERM_TOKEN_ENV__"); if (!token) return next(event);
    headers = { "content-type": "application/json", authorization: "Bearer " + token };
    queue = []; currentActivity={};loops.clear(); requests.clear(); toolCalls.clear();transcripts.clear();turnRecords.clear();backgroundTasks.clear();taskInfo.clear();historyBusy.clear();nativeNotifications.clear();
    try { await describe($); } catch { needsSnapshot = true; }
    if (!timersStarted) {
      timersStarted = true; $.clock.every(100, () => { void flush($); }); $.clock.every(200, () => { void poll($); });
    }
    return next(event);
  });
  on("session.end", async ($, event, next) => {
    sessionActive = false;
    for (const id of requests.keys()) resolveRequest(id);
    loops.clear(); toolCalls.clear(); needsSnapshot = true;
    emit({ type: "state", state: "offline", turnId: "" }); void flush($); return next(event);
  });
  on("turn.start", async ($, event, next) => {
    const agentId = event.agentId || "", streamId = (agentId ? agentId + ":" : "") + "live-" + (++sequence);
    if (!agentId) {
      const native = await $.session.id();
      if (native !== sessionId) { sessionId = native; queue = [];currentActivity={};nativeNotifications.clear();loops.clear();requests.clear();toolCalls.clear();transcripts.clear();turnRecords.clear();backgroundTasks.clear();taskInfo.clear();needsSnapshot = true; }
      emit({ type: "activity", value: {} }); emit({ type: "state", state: "running", turnId: event.turnId });
    } else { emit(taskEvent({ id: agentId }, "running", event.turnId)); void agents($); }
    loops.set(agentId, { streamId, turnId: event.turnId, running: true, startedAtMs:Date.now(),blocks:new Map() });
    const value={id:event.turnId,agentId,state:"inProgress",startedAtMs:Date.now(),timeSource:"observed"};
    turnRecords.set(agentId+":"+event.turnId,value);emit({type:"turn",value});
    return next(event);
  });
  on("turn.step", async function* ($, event, next) {
    const stream = next(event), loop = loopFor(event.agentId);
    for await (const chunk of stream) {
      if(loop&&["text","thinking","tool","input"].includes(chunk.kind)) {
        const key=event.index+":"+chunk.index;
        let item=loop.blocks.get(key);
        if(!item){item={id:chunk.kind==="tool"?(event.agentId?event.agentId+":":"")+chunk.id:loop.streamId+":"+key,kind:chunk.kind==="thinking"?"thinking":chunk.kind==="tool"||chunk.kind==="input"?"tool":"assistant",text:"",title:chunk.name||"",status:"running",turnId:loop.turnId,agentId:event.agentId||"",data:{stepIndex:event.index,blockIndex:chunk.index,startedAtMs:Date.now(),timeSource:"observed"}};loop.blocks.set(key,item);}
        if(chunk.kind==="text"||chunk.kind==="thinking")item.text=clipped(item.text+chunk.text);
        if(chunk.kind==="input") {item.data.inputJson=clipped((item.data.inputJson||"")+chunk.json);try{item.data.input=JSON.parse(item.data.inputJson);}catch{}}
        putItem({...item});
      }
      if(loop&&chunk.kind==="stop"){for(const [key,item] of loop.blocks)if(key.startsWith(event.index+":")){if(item.kind!=="tool"){item.status="completed";putItem({...item});}}}
      yield chunk;
    }
    return await stream.result;
  });
  on("turn.complete", async ($, event, next) => {
    const agentId = event.agentId || "", loop = loopFor(agentId);
    if (loop) loop.running = false;
    for (const [id, request] of requests) if (request.interaction.agentId === agentId && request.interaction.turnId === event.turnId) resolveRequest(id, true);
    const state = event.reason === "error" ? "failed" : event.reason === "aborted" || event.isAborted ? "interrupted" : event.reason === "refusal" ? "blocked" : "idle";
    if(loop)loop.state=state;
    for(const item of timeline(agentId).filter(row=>row.turnId===event.turnId&&row.status==="running")){putItem({...item,status:state==="idle"?"completed":state});}
    const value={...turnRecords.get(agentId+":"+event.turnId),id:event.turnId,agentId,state:state==="idle"?"completed":state,completedAtMs:Date.now(),durationMs:event.durationMs,usage:event.usage,timeSource:"nativeDuration"};
    turnRecords.set(agentId+":"+event.turnId,value);emit({type:"turn",value});
    if (agentId) { emit(taskEvent({ id: agentId }, state === "idle" ? "completed" : state, "", event.answer)); await agents($); }
    else {
      emit({ type: "activity", value: state === "idle" ? {} : { kind: state, message: state === "blocked" ? "Claude 拒绝了本次请求，请在电脑端查看详情" : state === "failed" ? "API 请求失败，本轮已结束" : "当前回合已中断" } });
      emit({ type: "state", state, turnId: "" });
    }
    return next(event);
  });
  on("classic.StopFailure", async ($, event, next) => {
    const message=clipped(event.error_details || event.error || "Claude 请求失败");
    if(event.agent_id)emit(taskEvent({id:event.agent_id},"failed","",message));
    else emit({ type: "activity", value: { kind: "failed", message } });
    return next(event);
  });
  // 只读取引擎 HTTP 状态，不读取或转发模型请求体、认证头和响应体。
  on("http.fetch", async ($, event, next) => {
    const result = await next(event);
    if (next.origin?.plugin === "engine" && (result.status === 429 || result.status >= 500) && main()?.running) {
      emit({ type: "activity", value: { kind: "apiError", message: "API 返回 HTTP " + result.status + "；等待 Claude 的后续处理或重试", status: result.status } });
    }
    if (next.origin?.plugin === "engine" && result.status >= 200 && result.status < 300 && main()?.running) emit({ type: "activity", value: {} });
    return result;
  });
  on("prompt.submit",async($,event,next)=>{if(event.origin?.kind==="task-notification"&&taskNotification(event.text)){nativeNotifications.add(event.text);if(nativeNotifications.size>64)nativeNotifications.delete(nativeNotifications.values().next().value);}return next(event);});
  on("classic.UserPromptSubmit", async ($, event, next) => { emit({ type: "settings", model: null, policy: event.permission_mode || "" });const notification=nativeNotifications.delete(event.prompt)||event.source==="system"&&taskNotification(event.prompt);if(event.prompt&&!notification)putItem({id:"user-"+(++sequence),kind:event.source==="system"?"activity":"user",title:event.source==="system"?"系统通知":"",text:clipped(event.prompt),status:"completed",turnId:"",agentId:"",data:{}});return next(event); });
  on("classic.PostModelSwitch", async ($, event, next) => { model = event.to_model; emit({ type: "settings", model, policy: null }); return next(event); });
  on("agent.spawn", async ($, event, next) => { const result = await next(event); await agents($); return result; });
  on("tool.call", async ($, event, next) => {
    const nativeId = event.tool_use_id || "call-" + (++sequence), agentId = event.agentId || "", loop = loopFor(agentId);
    const id = (agentId ? agentId + ":" : "") + nativeId;
    const startedAtMs=Date.now();let input=toolInput(event);
    toolCalls.set(nativeId, { tool: event.tool, input, agentId,itemId:id,permissionInput:JSON.stringify(input) });
    const upsert = (status, text, result) => {try{input=JSON.parse(toolCalls.get(nativeId)?.permissionInput||JSON.stringify(input));}catch{}putItem({ id, kind: "tool", title: event.tool, text: clipped(text), status, turnId: loop?.turnId || "", agentId,data:{type:"claudeTool",tool:event.tool,input:boundedData(input),result:boundedData(result),startedAtMs,...(status!=="running"?{completedAtMs:Date.now(),durationMs:Date.now()-startedAtMs,timeSource:"observed"}:{})} });};
    upsert("running", "");
    try {
      const result = await next(event); upsert(result.isError||result.deny ? "error" : "completed", toolSummary(result),result);backgroundResult(event.tool,input,result,id,agentId,startedAtMs);return result;
    } catch (error) { upsert("error", String(error)); throw error; }
    finally { for (const [requestId, request] of requests) if (request.toolCallId === nativeId) resolveRequest(requestId, true); toolCalls.delete(nativeId); }
  });
  on("classic.PreToolUse", async ($, event, next) => {
    if (!toolCalls.has(event.tool_use_id)) toolCalls.set(event.tool_use_id,{tool:event.tool||event.tool_name,agentId:event.agentId||event.agent_id||""});
    const input=toolInput(event);
    toolCalls.get(event.tool_use_id).permissionInput = JSON.stringify(input);
    const call=toolCalls.get(event.tool_use_id),id=(call.agentId?call.agentId+":":"")+event.tool_use_id;
    const item=timeline(call.agentId).find(row=>row.id===id);
    if(item)putItem({...item,data:{...item.data,input:boundedData(input)}});
    return next(event);
  });
  on("classic.PermissionRequest", async ($, event, next) => {
    const [toolCallId, call] = sourceFor(event), id = "permission-" + (++sequence);
    if (call) call.assigned = true;
    // 原生 hook 自身提供待答身份；工具关联只用于提前确认另一端已完成。
    const interaction = permissionInteraction(event, id, call);
    // 响应绑定当前 hook 的请求 ID；agent_id 直接来自原生事件。找不到工具 ID 时依赖 next.signal 失效清理。
    const request = { interaction, toolCallId };
    const answer = await waitResponse($, next, request);
    if (!answer) return next(event);
    try { return permissionResult(interaction, event, answer); }
    catch (error) { resolveRequest(id); emit({ type: "error", message: clipped(error) }); return next(event); }
  });
  on("classic.Elicitation", async ($, event, next) => {
    const id = "elicitation-" + (++sequence), nativeId = event.elicitation_id;
    const interaction = { id, kind: event.mode === "url" ? "url" : event.requested_schema?.properties&&Object.keys(event.requested_schema.properties).length===0?"confirmation":"form", title: "工具需要补充信息", detail: event.message || "", agentId: event.agent_id || "",
      turnId: loopFor(event.agent_id)?.turnId || "", blocking: true, data: { url: event.url, schema: event.requested_schema, serverName: event.mcp_server_name } };
    const request = { interaction, nativeId };
    const answer = await waitResponse($, next, request);
    if (!answer) return next(event);
    resolveRequest(id, true); return { action: answer.action, ...(answer.content ? { content: answer.content } : {}) };
  });
  on("classic.ElicitationResult", async ($, event, next) => {
    for (const [id, request] of requests) if (request.nativeId && request.nativeId === event.elicitation_id) resolveRequest(id, true);
    return next(event);
  });
  on("classic.Notification",async($,event,next)=>{emit({type:"notice",value:{id:"notification-"+(++sequence),title:event.title||"Claude 提示",message:clipped(event.message),severity:"info"}});return next(event);});
  on("session.receive",async($,event,next)=>{if(event.origin?.kind==="task-notification"&&!taskNotification(event.text))emit({type:"notice",value:{id:"task-notice-"+(++sequence),agentId:event.agentId||"",title:"任务通知",message:clipped(event.text),severity:"info"}});return next(event);});
  on("classic.CwdChanged",async($,event,next)=>{emit({type:"metadata",value:{cwd:event.new_cwd}});return next(event);});
  on("classic.PreCompact",async($,event,next)=>{putItem({id:"compact-"+(++sequence),kind:"activity",title:"正在压缩上下文",text:"",status:"running",turnId:main()?.turnId||"",agentId:"",data:{type:"contextCompaction"}});return next(event);});
  on("classic.PostCompact",async($,event,next)=>{const item=[...timeline("")].reverse().find(row=>row.data?.type==="contextCompaction"&&row.status==="running");if(item)putItem({...item,status:"completed",title:"上下文已压缩"});return next(event);});
}
