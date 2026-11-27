/* 原型演示控制：所有会话和操作只保存在页面内存，不调用产品接口。 */
(() => {
  const scenarios = {
    idle: "电脑与网页共享同一会话。试试模型、权限菜单和右侧会话列表。",
    running: "模拟分段输出。可以停止生成，也可以输入内容追加到同一个会话。",
    approval: "工具请求审批。点击查看详情、批准一次或拒绝，电脑端同步展示结果。",
    queued: "输出期间的追加消息单独显示，避免把“已接受”误认为“已执行”。",
    readonly: "允许查看，收起输入区。分享权限与 Agent 的工具执行权限分别展示。",
    offline: "断线保留会话内容和草稿，禁用操作；点击重连恢复，不自动重发。",
    error: "展示设置失败后的反馈，保留原来的模型和会话，支持重新选择。",
    ended: "会话已经结束，网页保留当前内容供查看，不能继续发送。",
    empty: "还没有可访问的 Agent 会话。用户在电脑终端使用 Agent 后自动出现。"
  };
  const presets = [
    { id:"sterm", title:"sTerm · Codex", agent:"codex", cwd:"D:\\workspace\\sTerm", model:"GPT-6.2 Sol", policy:"工作区写入", state:"idle" },
    { id:"api", title:"api-server · Claude", agent:"claude", cwd:"D:\\workspace\\api-server", model:"Claude Sonnet", policy:"默认审批", state:"approval" },
    { id:"docs", title:"文档整理 · Codex", agent:"codex", cwd:"D:\\workspace\\docs", model:"GPT-6.2 Astra", policy:"只读", state:"idle" }
  ];
  const fresh = () => presets.map(row => ({ ...row, custom:false, shareRule:"default", access:"control", extras:[], queue:[], modelNote:"", customText:"", approved:null }));
  let data = fresh(), active = "sterm", scenario = "idle", launching = false;
  let defaults={share:true,control:true}, launchTimer, revision=0;
  const streamTimers=new Map();
  const stopStream=id=>{clearInterval(streamTimers.get(id));streamTimers.delete(id);};
  const stopAllStreams=()=>{for(const timer of streamTimers.values())clearInterval(timer);streamTimers.clear();};
  const accessFor=rule=>rule==="default"?(defaults.share?(defaults.control?"control":"readonly"):"off"):rule;
  let previewView = new URLSearchParams(location.search).get("view") || "compare";
  if (!["mobile","browser","desktop","compare"].includes(previewView)) previewView = "compare";
  const frames = [...document.querySelectorAll("iframe")];
  const clone = value => JSON.parse(JSON.stringify(value));
  const current = () => data.find(row => row.id === active);
  const payload = () => ({ revision:++revision, scenario, active, sessions:clone(data), defaults:{...defaults}, launching, note:scenarios[scenario] });
  function broadcast() {
    const value = payload();
    for (const frame of frames) if (frame.contentWindow) frame.contentWindow.postMessage({ stPrototype:true, type:"state", value }, "*");
    document.getElementById("scenario-description").textContent = scenarios[scenario];
    document.getElementById("finish-demo").hidden=!["running","queued"].includes(current()?.state);
    for (const button of document.querySelectorAll("[data-scenario]")) button.setAttribute("aria-pressed", String(button.dataset.scenario === scenario));
  }
  function feedback(text) { document.getElementById("interaction-feedback").textContent = text; }
  function stream() {
    const row = current();
    if(row)stopStream(row.id);
    if (!row || !["running","queued"].includes(row.state)) return;
    const text = row.agent === "claude"
      ? "已确认 API 的返回格式。正在把参数校验收敛到 service 层，并保留现有错误码。接下来补充边界处理，最后运行受影响的检查。"
      : "已找到会话切换时草稿错位的原因。正在让草稿按会话保存，同时保留原有的只读权限检查。网页的消息布局独立于终端尺寸，调整手机宽度不会改变电脑的显示。";
    let index = 0;
    const go = () => {
      index = Math.min(text.length, index + 3);
      row.customText = text.slice(0,index); row.streamDone = index === text.length;
      broadcast();
      if (index === text.length) stopStream(row.id);
    };
    go(); streamTimers.set(row.id,setInterval(go,120));
  }
  function setScenario(name) {
    if (!scenarios[name]) return;
    stopAllStreams();clearTimeout(launchTimer);launching=false;defaults={share:true,control:true};
    data=fresh();active="sterm";scenario=name;
    const row=current();row.state=["readonly","empty"].includes(name)?"idle":name;
    if(name==="readonly"){row.access="readonly";row.shareRule="readonly";}
    if(name==="queued") row.queue=["请保留原来的会话切换方式。"];
    if(name==="error") row.modelNote="模型切换失败：当前服务商暂不可用。仍使用 GPT-6.2 Sol。";
    broadcast();
    if(name==="running"||name==="queued") stream();
    feedback("当前演示："+document.querySelector('[data-scenario="'+name+'"]').textContent);
  }
  function setView(name) {
    previewView=name;document.body.dataset.view=name;
    for(const b of document.querySelectorAll(".view-tabs button")) b.setAttribute("aria-pressed",String(b.dataset.view===name));
    const url=new URL(location.href);url.searchParams.set("view",name);history.replaceState(null,"",url);
  }
  function handle(action,fromDesktop=false) {
    const row=current();
    if(action.type==="select") {
      if(!data.some(item=>item.id===action.id))return;
      active=action.id;broadcast();feedback("已切换到 "+current().title+"，各会话草稿独立保留。");return;
    }
    if(action.type==="scenario"){setScenario(action.value);return;}
    if(action.type==="launch") {
      stopAllStreams();clearTimeout(launchTimer);launching=true;scenario="empty";broadcast();feedback("电脑终端：输入 codex，演示自动识别与接入。");
      launchTimer=setTimeout(()=>{data=fresh();for(const item of data)item.access=accessFor(item.shareRule);active="sterm";scenario="idle";launching=false;broadcast();feedback("Agent 已自动出现在网页会话列表，无需填写会话 ID。");},1700);
      return;
    }
    if(!row)return;
    const canControl=fromDesktop||row.access==="control";
    if(action.type==="defaults"){
      if(!["share","control"].includes(action.key))return;
      defaults[action.key]=Boolean(action.value);
      for(const item of data)if(item.shareRule==="default")item.access=accessFor("default");
      broadcast();feedback("默认分享规则已更新，单独设置的会话保持自己的权限。");return;
    }
    if(action.type==="share") {
      const target=data.find(item=>item.id===action.id);if(!target)return;
      target.shareRule=action.value;target.access=accessFor(action.value);
      broadcast();feedback(action.value==="off"?"已停止分享 "+target.title:"分享权限已更新；电脑终端始终可操作。");return;
    }
    if(action.type==="model"||action.type==="policy") {
      if(["offline","ended","approval","running","queued"].includes(row.state)||row.access!=="control")return;
      if(action.type==="model"){row.model=action.value;row.modelNote="";feedback("模型已切换为 "+row.model+"，下次请求使用该模型。");}
      else {row.policy=action.value;feedback("当前权限已更新为「"+row.policy+"」。");}
      broadcast();return;
    }
    if(action.type==="approval") {
      if(row.state!=="approval"||!canControl)return;
      row.approved=Boolean(action.allow);row.state="idle";scenario="idle";
      row.extras.push({role:"system",text:"你在"+(fromDesktop?"电脑终端":"网页")+(action.allow?"批准":"拒绝")+"了这次操作。"});
      row.extras.push({role:"assistant",text:action.allow?"已完成这次操作，并继续检查了相关结果。原生终端和其他网页的审批已同步关闭。":"已取消这次操作，没有继续执行。你可以补充要求后继续当前会话。"});
      broadcast();feedback(action.allow?"已批准一次；其余端的待审批已同步关闭。":"已拒绝；当前审批已结束。");return;
    }
    if(action.type==="stop") {
      if(!canControl||!["running","queued","approval"].includes(row.state))return;
      stopStream(row.id);row.state="idle";scenario="idle";row.stopped=true;
      row.extras.push({role:"system",text:"已停止本轮生成。会话保留，可以继续。"});
      broadcast();feedback("已停止当前回合；没有结束 Agent 会话。");return;
    }
    if(action.type==="reconnect") {if(row.state==="offline"){row.state="idle";scenario="idle";}broadcast();feedback("连接已恢复，草稿仍在；没有自动重发消息。");return;}
    if(action.type==="dismissError") {row.modelNote="";row.state="idle";scenario="idle";broadcast();return;}
    if(action.type==="send") {
      const text=String(action.text||"").trim();if(!text||!canControl||(!fromDesktop&&row.state==="offline")||row.state==="ended"||scenario==="empty")return;
      if(["running","queued","approval"].includes(row.state)){row.queue.push(text);if(row.state!=="approval")row.state="queued";feedback("消息已追加；由当前 Agent 安排执行。");broadcast();return;}
      row.extras.push({role:"user",text});row.state="running";row.stopped=false;row.custom=true;row.customText="";scenario="running";broadcast();stream();feedback("已发送到 "+row.title+"，演示实时输出。");return;
    }
    if(action.type==="finish") {
      stopStream(row.id);row.state="idle";scenario="idle";
      const text=row.agent==="claude"?"已经完成参数校验调整，相关测试全部通过。":"已经完成会话草稿修复，相关检查通过。电脑端和网页均可继续本会话。";
      row.extras.push({role:"assistant",text});row.customText="";row.custom=false;
      for(const queued of row.queue){row.extras.push({role:"user",text:queued},{role:"assistant",text:"已按追加要求处理，并保留了原来的使用方式。"});}
      row.queue=[];broadcast();feedback("本轮已完成。");return;
    }
    if(action.type==="end") {if(!canControl)return;stopStream(row.id);row.state="ended";scenario="ended";broadcast();feedback("演示：原生 Agent 会话已结束，其余会话仍然可用。");return;}
    if(action.type==="copy")feedback("已演示复制内容。原型不会读取真实会话。");
  }
  for(const frame of frames)frame.addEventListener("load",broadcast);
  window.addEventListener("message", event=>{
    if(!frames.some(frame=>frame.contentWindow===event.source)||!event.data?.stPrototype)return;
    if(event.data.type==="ready"){broadcast();return;}
    if(event.data.type==="action")handle(event.data.action,event.source===document.getElementById("desktop-preview").contentWindow);
  });
  for(const button of document.querySelectorAll(".view-tabs button"))button.onclick=()=>setView(button.dataset.view);
  for(const button of document.querySelectorAll("[data-scenario]"))button.onclick=()=>setScenario(button.dataset.scenario);
  document.getElementById("reset-demo").onclick=()=>setScenario(scenario);
  document.getElementById("finish-demo").onclick=()=>handle({type:"finish"});
  document.getElementById("simulate-launch").onclick=()=>handle({type:"launch"});
  document.getElementById("phone-width").onchange=event=>document.documentElement.style.setProperty("--phone-width",event.target.value+"px");
  setView(previewView);broadcast();
})();
