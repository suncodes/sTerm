/* 评审原型：控件只修改演示状态，不访问终端、网络或凭据。 */
(() => {
  const $=id=>document.getElementById(id);
  const icon=(name,cls="")=>'<svg class="icon '+cls+'" aria-hidden="true"><use href="#i-'+name+'"/></svg>';
  const esc=value=>String(value??"").replace(/[&<>"']/g,char=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[char]));
  const desktop=new URLSearchParams(location.search).get("surface")==="desktop";
  $("web-app").hidden=desktop;$("desktop-app").hidden=!desktop;
  let data={scenario:"idle",active:"sterm",sessions:[],launching:false}, previousId="", sheetType="", beforeSheet=null;
  let messageKey="",tabsKey="",desktopKey="",settingsKey="",settingsPage="remote",settingsTab="permissions",localMode="agent",toastTimer,localDrafts=new Map();
  let keyboardComposing=false, fontSize=13;
  const label={idle:"空闲",running:"运行中",queued:"运行中",approval:"等待审批",readonly:"只读",offline:"连接中断",error:"操作失败",ended:"已结束"};
  const current=()=>data.sessions.find(row=>row.id===data.active);
  const isEmpty=()=>data.scenario==="empty";
  const stateOf=row=>row?.access==="readonly"?"只读":label[row?.state]||"空闲";
  const writable=row=>Boolean(row&&row.access==="control"&&!["offline","ended"].includes(row.state)&&!isEmpty());
  const busy=row=>["running","queued","approval"].includes(row?.state);
  function send(action){
    if(parent!==window) parent.postMessage({stPrototype:true,type:"action",action},"*");
  }
  function toast(text){
    const target=$(desktop?"desktop-toast":"toast");target.textContent=text;target.hidden=false;
    clearTimeout(toastTimer);toastTimer=setTimeout(()=>target.hidden=true,2300);
  }
  function tool(title,body,kind="file",status="已完成"){
    return '<details class="tool-card"><summary>'+icon("chevron","tool-arrow")+icon(kind)+'<span>'+title+'</span><span class="tool-state">'+status+'</span></summary><div class="tool-body">'+body+'</div></details>';
  }
  function user(text){return '<article class="message user-message"><div class="message-heading">你</div><div class="message-text">'+esc(text).replace(/\n/g,"<br>")+'</div></article>';}
  function assistant(text,row){
    return '<article class="message agent-message"><div class="message-heading"><span class="agent-avatar">'+(row.agent==="claude"?"C":"›_")+'</span>'+ (row.agent==="claude"?"Claude Code":"Codex")+'<small>'+esc(row.model)+'</small></div><div class="message-body">'+text+'</div></article>';
  }
  function baseline(row){
    if(row.agent==="claude"){
      return user("帮我检查登录接口的参数校验，保留现有错误码。")+
      assistant('<details class="thought-summary"><summary>'+icon("chevron")+'思考摘要 · 已完成</summary><p>先核对参数的入口与错误映射，再检查相关测试。</p></details>'+
      '<div class="message-text"><p>已找到登录接口和现有错误处理。我会保持响应格式，只补充缺失的参数校验。</p></div>'+
      tool("读取 LoginController 与 LoginService","已读取 2 个文件\nLoginController.java\nLoginService.java","file")+
      '<div class="message-text"><p>空字符串和缺失字段目前走了不同路径，需要统一处理。</p></div>',row);
    }
    return user("修复切换会话后草稿丢失的问题，保持现有终端交互。")+
      assistant('<details class="thought-summary"><summary>'+icon("chevron")+'思考摘要 · 已完成</summary><p>草稿应随会话保存。会话切换只替换显示目标，不能清除未发送内容。</p></details>'+
      tool("读取会话切换与输入逻辑","已读取 3 个文件\nsrc/remote/client.ts\nsrc/remote/history.ts\nsrc/terminal/session.ts","file")+
      (["approval","running","queued"].includes(row.state)?'<div class="message-text"><p>已定位问题，正在调整草稿的保存和恢复时机。</p></div>':
      '<div class="message-text"><p>已修复。切换会话会保留各自的草稿，断线和恢复也不会自动发送。</p></div>'+
      '<div class="changed-file">'+icon("file")+'<code>src/remote/client.ts</code><span class="diff-add">+24</span><span class="diff-minus">−8</span></div>'+
      '<div class="changed-file">'+icon("file")+'<code>src/remote/history.ts</code><span class="diff-add">+12</span><span class="diff-minus">−3</span></div>'+
      '<div class="result-check">'+icon("check")+'相关检查通过 · 2 个文件已更新</div>'+
      '<div class="message-actions"><button data-copy-message>'+icon("copy")+'复制</button><button data-message-detail>'+icon("code")+'查看变更</button></div>'),row);
  }
  function messagesHtml(row){
    if(isEmpty())return '<div class="empty-conversation">'+icon("monitor")+'<h2>还没有共享的 Agent 会话</h2><p>在电脑端的 sTerm 终端中使用 Claude 或 Codex，接入后会话会显示在这里。</p><small>可访问的会话由电脑端分享设置决定</small></div>';
    if(row.access==="off")return '<div class="empty-conversation">'+icon("lock")+'<h2>此会话已停止分享</h2><p>电脑端仍可继续使用。你可以切换到其他已分享会话。</p><button class="subtle-button" data-open="sessions">切换会话</button></div>';
    let html='<div class="day-label">今天 · 同一个会话</div>'+baseline(row);
    for(const item of row.extras||[]){
      if(item.role==="user")html+=user(item.text);
      else if(item.role==="assistant")html+=assistant('<div class="message-text"><p>'+esc(item.text)+'</p></div>',row);
      else html+='<div class="result-check message">'+icon("check")+esc(item.text)+'</div>';
    }
    if(["running","queued"].includes(row.state)||row.customText&&row.stopped)
      html+=assistant('<div class="message-text"><p><span id="live-output">'+esc(row.customText||"正在整理本轮变更…")+'</span>'+(busy(row)?'<span class="stream-caret"></span>':"")+'</p></div>',row);
    if(row.state==="ended")html+='<div class="day-label">会话已结束 · 内容已保留</div>';
    return html;
  }
  function renderTabs(row){
    const rows=isEmpty()?[]:data.sessions.filter(item=>item.access!=="off");
    const key=JSON.stringify([data.active,rows.map(item=>[item.id,item.state,item.title,item.access])]);
    if(key===tabsKey)return;tabsKey=key;
    $("session-tabs").innerHTML=rows.map(item=>'<button class="session-tab" role="tab" aria-selected="'+(item.id===data.active)+'" data-select-session="'+item.id+'"><span class="status-dot" data-state="'+item.state+'"></span><strong>'+esc(item.title)+'</strong><span class="tab-status">'+stateOf(item)+'</span></button>').join("");
    $("session-count").textContent=rows.length;
    $("current-title").textContent=isEmpty()?"等待分享会话":row.title;
    $("current-dot").hidden=isEmpty();$("current-dot").dataset.state=row?.state||"idle";
    $("current-state").textContent=isEmpty()?"":stateOf(row);
  }
  function renderApproval(row){
    const show=row.state==="approval"&&!isEmpty()&&row.access!=="off"&&localMode==="agent";
    $("approval-dock").hidden=!show;if(!show)return;
    const claude=row.agent==="claude";
    const key=row.id+row.state+row.access;
    if($("approval-dock").dataset.key===key)return;
    $("approval-dock").dataset.key=key;
    $("approval-dock").innerHTML='<div class="approval-card"><div class="approval-heading">'+icon("lock")+'需要你的审批<small>'+(claude?"修改文件":"执行命令")+'</small></div><p>'+(claude?"补充登录参数校验，修改工作区内的 1 个文件。":"运行受影响的回归测试，确认会话切换行为。")+'</p><div class="approval-target">'+(claude?"src/service/LoginService.java":"npm run test:remote")+'<span class="command-path">'+esc(row.cwd)+'</span></div><div class="approval-actions"><button class="details-link" data-open="approval-detail">查看详情</button><button data-approval="deny" '+(!writable(row)?"disabled":"")+'>拒绝</button><button class="approve" data-approval="allow" '+(!writable(row)?"disabled":"")+'>批准一次</button></div></div>';
  }
  function render(){
    const row=current();if(!row)return;
    const changed=previousId!==row.id;
    if(changed){
      if(previousId)localDrafts.set(previousId,$("draft").value);
      $("draft").value=localDrafts.get(row.id)||"";
      previousId=row.id;messageKey="";closeSheet(false);
    }
    if(sheetType==="approval-detail"&&row.state!=="approval")closeSheet();
    if(["models","policy"].includes(sheetType)&&(!writable(row)||busy(row)))closeSheet();
    if(desktop){renderDesktop(row);return;}
    renderTabs(row);
    $("session-path").textContent=isEmpty()?"仅显示已接入且获准分享的会话":row.cwd;
    $("share-access").textContent=isEmpty()?"":row.access==="readonly"?"只读":row.access==="off"?"已停止分享":"双端可操作";
    $("share-access").dataset.readonly=row.access!=="control";
    $("connection").textContent=row.state==="offline"?"重连中":"已连接";
    $("connection").dataset.offline=String(row.state==="offline");
    $("agent-settings").hidden=isEmpty()||row.access==="off"||localMode==="terminal";
    $("current-model").textContent=row.model;
    $("current-policy").textContent=row.policy;
    const settingsWritable=writable(row)&&!busy(row);
    $("model-button").disabled=!settingsWritable;$("policy-button").disabled=!settingsWritable;
    $("settings-current-state").innerHTML='<span class="status-dot" data-state="'+row.state+'"></span><span>'+stateOf(row)+'</span>';
    $("connection-banner").hidden=row.state!=="offline";
    $("operation-error").hidden=!row.modelNote;
    $("operation-error").querySelector("span").textContent=row.modelNote||"";
    const signature=JSON.stringify([row.id,row.state,row.extras,row.access,row.stopped,isEmpty()]);
    if(messageKey!==signature){
      const scroll=$("conversation"),wasBottom=scroll.scrollHeight-scroll.scrollTop-scroll.clientHeight<60;
      const details=[...$("messages").querySelectorAll("details")].map(item=>item.open);
      $("messages").innerHTML=messagesHtml(row);
      [...$("messages").querySelectorAll("details")].forEach((item,index)=>{if(!changed&&details[index])item.open=true;});
      messageKey=signature;
      if(changed)scroll.scrollTop=0;
      else if(wasBottom&&row.extras?.length)scroll.scrollTop=scroll.scrollHeight;
    }
    const live=$("live-output");if(live)live.textContent=row.customText||"正在整理本轮变更…";
    renderApproval(row);
    const available=writable(row)&&localMode==="agent";
    $("composer").hidden=!available;
    $("composer-unavailable").hidden=available;
    let disabledHtml="";
    if(isEmpty())disabledHtml=icon("monitor")+'<span>在电脑终端启动 Agent 后，从会话列表选择</span>';
    else if(row.access==="off")disabledHtml=icon("lock")+'<span>此会话已取消分享</span><button data-open="sessions">其他会话</button>';
    else if(row.state==="offline")disabledHtml=icon("wifi")+'<span>草稿已保留，恢复连接后可继续发送</span>';
    else if(row.state==="ended")disabledHtml=icon("stop")+'<span>会话已结束</span><button data-open="sessions">切换会话</button>';
    else if(row.access==="readonly")disabledHtml=icon("lock")+'<span>当前为只读，电脑端可修改分享权限</span>';
    else disabledHtml=icon("terminal")+'<span>终端画面示意</span><button id="return-agent-inline">返回 Agent 操作</button>';
    $("composer-unavailable").innerHTML=disabledHtml;
    $("activity-bar").hidden=isEmpty()||row.access==="off"||localMode==="terminal";
    $("activity-bar").querySelector(".status-dot").dataset.state=row.state;
    let activity=row.agent==="claude"?"本轮已完成":"本轮完成 · 修改 2 个文件";
    if(row.state==="running"||row.state==="queued")activity=row.streamDone?"演示输出已显示 · 可停止或追加消息":"正在生成回复…";
    if(row.state==="approval")activity="等待审批 · 电脑端也可以处理";
    if(row.state==="offline")activity="显示断线前的内容";
    if(row.state==="ended")activity="会话已结束";
    if(row.stopped&&row.state==="idle")activity="已停止本轮 · 会话可继续";
    $("activity-text").textContent=activity;
    $("stop-turn").hidden=!busy(row)||!writable(row);
    $("queue-bar").hidden=!row.queue?.length||isEmpty()||localMode==="terminal";
    $("queue-text").textContent="已追加 "+(row.queue?.length||0)+" 条消息，等待 Agent 处理";
    $("draft").placeholder=busy(row)?"补充要求，追加到当前会话…":"继续这个会话…";
    $("input-hint").textContent=busy(row)?"由当前 Agent 安排执行":"电脑端与网页均可操作";
    $("send-button").textContent=busy(row)?"追加":"发送";
    $("send-button").disabled=!available||!$("draft").value.trim();
    $("conversation").hidden=localMode==="terminal";
    $("terminal-view").hidden=localMode!=="terminal";
    $("terminal-output").textContent="PS "+row.cwd+"> "+row.agent+"\n\n  › 修复切换会话后草稿丢失的问题\n\n  • 已读取相关文件\n  • "+(busy(row)?"正在处理…":"已完成，相关检查通过。")+"\n\n  "+row.model+" · "+row.policy;
    for(const button of document.querySelectorAll("[data-mode]"))button.setAttribute("aria-pressed",String(button.dataset.mode===localMode));
    if(sheetType==="sessions"||sheetType==="approval-detail"){
      const focus=document.activeElement, selector=focus?.dataset?.approval?'[data-approval="'+focus.dataset.approval+'"]':focus?.dataset?.selectSession?'[data-select-session="'+focus.dataset.selectSession+'"]':null;
      fillSheet(sheetType);
      if(selector)$("sheet").querySelector(selector)?.focus();
    }
  }
  function heading(title){return '<div class="sheet-heading"><h2 id="sheet-title">'+title+'</h2><button class="touch-button" data-close-sheet aria-label="关闭面板">'+icon("close")+'</button></div>';}
  function fillSheet(type){
    const row=current();if(!row)return;
    let html="";
    if(type==="sessions"){
      const rows=isEmpty()?[]:data.sessions.filter(item=>item.access!=="off");
      html=heading("会话")+'<p class="sheet-description">电脑端正在使用的 Agent，接入后自动出现在这里。</p><div class="sheet-group-label">工作电脑 · '+rows.length+' 个共享会话</div>'+
      rows.map(item=>'<button class="sheet-list-button" data-select-session="'+item.id+'" aria-current="'+(item.id===data.active)+'"><span><strong>'+esc(item.title)+'</strong><small>'+esc(item.cwd)+'</small><small class="sheet-row-meta"><i class="status-dot" data-state="'+item.state+'"></i>'+stateOf(item)+' · '+(item.access==="readonly"?"只读":"允许操作")+'</small></span>'+(item.id===data.active?icon("check","selected-check"):"")+'</button>').join("")+
      '<p class="sheet-footnote">'+(rows.length?"切换只改变当前网页，不会切换其他终端。":"暂无共享会话。请在电脑端使用 Agent，并检查分享设置。")+'</p>';
    } else if(type==="models"){
      const options=row.agent==="claude"?[["Claude Sonnet","日常编程与修改"],["Claude Opus","复杂任务与深入分析"],["Claude Haiku","快速响应"]]:[["GPT-6.2 Sol","日常编程与快速迭代"],["GPT-6.2 Astra","复杂修改与深入分析"],["GPT-6.1 Sol","兼顾速度与推理"]];
      html=heading("切换模型")+'<p class="sheet-description">选择后用于后续请求，当前会话和历史保留。</p><div role="radiogroup" aria-label="可用模型">'+options.map(([value,detail])=>'<button class="sheet-list-button" role="radio" aria-checked="'+(row.model===value)+'" data-model="'+value+'"><span><strong>'+value+'</strong><small>'+detail+'</small></span>'+(row.model===value?icon("check","selected-check"):"")+'</button>').join("")+'</div><p class="sheet-footnote">示例模型列表。实际选项由当前 Agent 提供。</p>';
    } else if(type==="policy"){
      const options=row.agent==="claude"?[["默认审批","需要权限时，显示批准 / 拒绝操作。"],["Plan","先分析和规划，不直接修改项目。"]]:[["只读","允许查看项目；需要写入或执行时请求审批。"],["工作区写入","允许修改当前工作区；需要额外权限时审批。"]];
      html=heading("Agent 权限")+'<p class="sheet-description">这是 Agent 的执行范围，与网页是否允许输入分别设置。</p><div role="radiogroup" aria-label="Agent 权限">'+options.map(([value,detail])=>'<button class="sheet-list-button" role="radio" aria-checked="'+(row.policy===value)+'" data-policy="'+value+'"><span><strong>'+value+'</strong><small>'+detail+'</small></span>'+(row.policy===value?icon("check","selected-check"):"")+'</button>').join("")+'</div><p class="sheet-footnote">只显示当前 Agent 支持的操作。演示不修改实际权限。</p>';
    } else if(type==="more"){
      html=heading("会话选项")+
      '<button class="menu-item" data-open="sessions">'+icon("list")+'所有会话<span class="menu-value">切换</span></button>'+
      '<button class="menu-item" data-open="session-info">'+icon("monitor")+'会话信息<span class="menu-value">'+(row.agent==="claude"?"Claude":"Codex")+'</span></button>'+
      '<button class="menu-item" data-open="display">'+icon("settings")+'显示设置<span class="menu-value">消息字号</span></button>'+
      '<button class="menu-item" data-copy-link>'+icon("copy")+'复制会话链接</button>'+
      '<button class="menu-item" data-reconnect>'+icon("refresh")+'重新连接</button>'+
      '<button class="menu-item danger" data-open="end-confirm" '+(!writable(row)?"disabled":"")+'>'+icon("stop")+'结束 Agent 会话</button>';
    } else if(type==="approval-detail"){
      html=heading("审批详情")+'<p class="sheet-description">'+(row.agent==="claude"?"拟修改以下文件，补充参数校验。":"Agent 请求执行以下命令。批准只针对当前这一次。")+'</p>'+
      (row.agent==="claude"?'<pre class="detail-code"><span class="dim">src/service/LoginService.java</span>\n\n<span class="add">+ if (request.getUsername().isBlank()) {\n+     throw new BusinessException(INVALID_PARAM);\n+ }</span></pre>':'<pre class="detail-code">npm run test:remote\n\n<span class="dim">工作目录</span>\n'+esc(row.cwd)+'\n\n<span class="dim">目的</span>\n验证会话切换、草稿与权限回归。</pre>')+
      '<p class="sheet-description">也可以在电脑端审批。任一侧处理后，其他页面关闭此待办。</p><div class="confirm-actions"><button class="subtle-button" data-close-sheet>返回</button><button class="subtle-button" data-approval="deny" '+(!writable(row)?"disabled":"")+'>拒绝</button><button class="subtle-button approve-detail" data-approval="allow" '+(!writable(row)?"disabled":"")+'>批准一次</button></div>';
    } else if(type==="changes"){
      html=heading("文件变更")+'<p class="sheet-description">src/remote/client.ts</p><pre class="detail-code"><span class="dim">@@ 切换会话时保存草稿</span>\n<span class="remove">- draft.value = "";</span>\n<span class="add">+ drafts.set(currentId, draft.value);\n+ draft.value = drafts.get(nextId) ?? "";</span>\n\n<span class="dim">2 个文件 · +36 −11</span></pre>';
    } else if(type==="queue"){
      html=heading("已追加的消息")+'<p class="sheet-description">已送到当前会话，等待 Agent 按其队列规则处理。</p>'+
      row.queue.map((text,index)=>'<div class="detail-code">'+(index+1)+'. '+esc(text)+'</div>').join("")+
      '<p class="sheet-footnote">停止当前生成不会保证清空原生队列。</p>';
    } else if(type==="end-confirm"){
      html=heading("结束 Agent 会话？")+'<p class="sheet-description">将停止「'+esc(row.title)+'」在电脑端运行的 Agent。当前回合与后续执行会结束，网页保留已显示的内容。</p><p class="sheet-description">如果只是想中断本轮回复，请使用「停止生成」。</p><div class="confirm-actions"><button class="subtle-button" data-close-sheet>取消</button><button class="danger-button" data-end-confirm>结束会话</button></div>';
    } else if(type==="session-info"){
      html=heading("会话信息")+'<div class="detail-code">会话　'+esc(row.title)+'\n电脑　工作电脑\n目录　'+esc(row.cwd)+'\nAgent　'+(row.agent==="claude"?"Claude Code":"Codex")+'\n模型　'+esc(row.model)+'\n权限　'+esc(row.policy)+'\n网页　'+(row.access==="readonly"?"只读":"允许操作")+'</div><p class="sheet-description">原生终端与网页操作同一会话，电脑端始终可输入。</p>';
    } else if(type==="display"){
      html=heading("显示设置")+'<p class="sheet-description">只调整网页消息，不改变电脑终端的宽度或字号。</p><div class="menu-item">消息字号<span class="menu-value"><button class="subtle-button" data-font="-1">−</button> <output id="font-output">'+fontSize+' px</output> <button class="subtle-button" data-font="1">+</button></span></div>';
    }
    $("sheet").innerHTML=html;
  }
  function openSheet(type){
    if(!sheetType)beforeSheet=document.activeElement;
    sheetType=type;$("sheet-layer").dataset.sheet=type;$("sheet-layer").hidden=false;fillSheet(type);
    requestAnimationFrame(()=>$("sheet").querySelector("button")?.focus());
  }
  function closeSheet(focus=true){
    $("sheet-layer").hidden=true;sheetType="";
    if(focus&&beforeSheet?.isConnected)beforeSheet.focus();
  }
  function setMode(mode){localMode=mode;closeSheet();render();}
  function renderDesktop(row){
    const nextSettingsKey=JSON.stringify([data.defaults,isEmpty(),data.sessions.map(item=>[item.id,item.title,item.state,item.shareRule,item.access])]);
    if(!$("desktop-settings-overlay").hidden&&settingsKey!==nextSettingsKey){renderSettings();settingsKey=nextSettingsKey;}
    const key=JSON.stringify([row.id,row.state,row.model,row.policy,row.access,data.scenario,data.launching,row.customText,row.extras]);
    if(desktopKey===key)return;desktopKey=key;
    $("desktop-tabs").innerHTML=(isEmpty()?[]:data.sessions).map(item=>'<button class="desktop-tab" data-select-session="'+item.id+'" aria-selected="'+(item.id===row.id)+'"><span class="status-dot" data-state="'+item.state+'"></span><span>'+esc(item.title)+'</span><small>×</small></button>').join("")+'<button class="icon-button" id="desktop-new-shell" aria-label="新建终端">＋</button>';
    const strip=$("desktop-share-strip");
    strip.hidden=isEmpty();
    strip.querySelector("span").textContent=row.access==="off"?"Agent 已接入 · 当前未分享":row.access==="readonly"?"Agent 已接入 · 网页只读 · 电脑端可继续输入":"Agent 已接入 · 网页允许操作 · 电脑端可继续输入";
    let html='<span class="dim">PowerShell 7 · '+esc(row.cwd)+'</span>\n\n<span class="blue">PS '+esc(row.cwd)+'&gt;</span> ';
    if(isEmpty()){
      html+=data.launching?'<span class="blue">codex</span>\n\n<span class="dim">正在连接当前 Agent…</span>':'<span class="native-caret"></span>\n\n<span class="dim">在这里照常输入 claude 或 codex。\nAgent 接入后，网页会自动显示共享的活跃会话。</span>';
    } else {
      html+=row.agent+'\n\n<span class="dim">'+(row.agent==="claude"?"Claude Code":"Codex")+' · '+esc(row.model)+'</span>\n\n  › '+(row.agent==="claude"?"检查登录接口的参数校验，保留现有错误码":"修复切换会话后草稿丢失的问题")+'\n\n';
      if(row.state==="approval")html+='<span class="gold">  ? '+(row.agent==="claude"?"允许修改 LoginService.java？":"允许执行 npm run test:remote？")+'</span>\n\n  <span class="green">1. 批准一次</span>    2. 拒绝\n\n<span class="dim">  也可以在网页端处理本次审批。</span>';
      else if(["running","queued"].includes(row.state))html+='<span class="native-tool">  • 已读取相关文件</span>\n\n  '+esc(row.customText||"正在整理本轮修改…")+'\n\n<span class="dim">  Esc 中断 · 输入内容可追加</span>';
      else if(row.state==="ended")html+='<span class="dim">  Agent 已退出。</span>\n\n<span class="blue">PS '+esc(row.cwd)+'&gt;</span> <span class="native-caret"></span>';
      else html+='<span class="native-tool">  • 已读取 3 个文件</span>\n\n  已修复草稿保存与恢复的时机。\n  切换会话后，未发送的内容会继续保留。\n\n<span class="green">  ✓ 相关检查通过 · 修改 2 个文件</span>\n\n'+(row.extras?.some(item=>item.role==="system")?'<span class="dim">  '+esc(row.extras.filter(item=>item.role==="system").at(-1).text)+'</span>':"");
    }
    if(!isEmpty()&&row.extras?.length){
      html+='\n\n<span class="dim">  ── 后续对话 ──</span>';
      for(const item of row.extras){
        html+='\n\n'+(item.role==="user"?'<span class="blue">  › '+esc(item.text)+'</span>':item.role==="system"?'<span class="dim">  '+esc(item.text)+'</span>':'  '+esc(item.text));
      }
    }
    $("desktop-terminal-screen").innerHTML=html;
    $("native-input").placeholder=isEmpty()?"使用顶部按钮演示启动 Agent":row.state==="approval"?"等待审批，也可以补充要求…":row.state==="ended"?"原生 Agent 已结束":"继续这个会话…";
    $("native-input").disabled=isEmpty()||row.state==="ended";
    $("native-form").querySelector("button").disabled=$("native-input").disabled;
    const nativeControls=$("native-controls");
    nativeControls.hidden=!busy(row);
    const controlsKey=row.id+row.state;
    if(nativeControls.dataset.key!==controlsKey){
      nativeControls.dataset.key=controlsKey;
      nativeControls.innerHTML=(row.state==="approval"?'<button type="button" data-approval="allow">1. 批准一次</button><button type="button" data-approval="deny">2. 拒绝</button>':'')+'<button type="button" data-native-stop>Esc · 停止本轮</button>';
    }
    $("native-model-hint").textContent=row.model+" · "+row.cwd;
    $("desktop-status-right").textContent=isEmpty()?(data.launching?"正在接入 Agent…":"远程访问已开启"):row.access==="off"?"Agent 已接入 · 未分享":row.access==="readonly"?"Agent 已接入 · 网页只读":"Agent 已接入 · 1 个网页在线";

  }
  function renderSettings(){
    const target=$("desktop-settings-content");
    if(settingsPage==="appearance"){
      target.innerHTML='<h2 class="settings-page-title">外观</h2><p class="settings-page-subtitle">沿用当前 sTerm 的深色主题、字体与控件。</p><div class="settings-row"><span>主题<small>与终端、设置、远程网页保持一致</small></span><span>深色</span></div><div class="settings-row"><span>界面字体</span><span>Segoe UI · 12 px</span></div>';return;
    }
    if(settingsPage==="integration"){
      target.innerHTML='<h2 class="settings-page-title">集成</h2><p class="settings-page-subtitle">正常使用终端时，自动连接支持的 Agent 会话。</p><label class="settings-row"><span>自动接入终端中的 Agent<small>支持 Claude Code 与 Codex；不更换原生终端界面。</small></span><input type="checkbox" class="settings-switch" checked data-integration></label><div class="settings-row"><span>Claude Code<small>当前版本支持 · 已接入</small></span><span>可用</span></div><div class="settings-row"><span>Codex<small>当前版本支持 · 已接入</small></span><span>可用</span></div><p class="settings-note">'+icon("monitor")+'原型展示自动接入的目标体验。兼容范围和启用方式需在实施时验证。</p>';return;
    }
    if(settingsPage==="about"){target.innerHTML='<h2 class="settings-page-title">sTerm</h2><p class="settings-page-subtitle">轻量终端 · Agent 会话 · 远程访问</p><p class="terminal-demo-note">此页为交互原型，使用模拟数据。未连接本机终端或真实 Agent。</p>';return;}
    const rows=isEmpty()?[]:data.sessions;
    const defaults=data.defaults||{share:true,control:true};
    const defaultLabel=!defaults.share?"不分享":defaults.control?"可操作":"只读";
    target.innerHTML='<div class="settings-page-title">远程访问<input type="checkbox" class="settings-switch" checked aria-label="启用远程访问演示" data-remote-toggle></div><p class="settings-page-subtitle">在手机与浏览器继续使用电脑中的会话。</p>'+
      '<div class="settings-subtabs" role="tablist" aria-label="远程设置"><button data-settings-tab="connection" role="tab" aria-selected="'+(settingsTab==="connection")+'">连接</button><button data-settings-tab="permissions" role="tab" aria-selected="'+(settingsTab==="permissions")+'">会话权限</button><button data-settings-tab="public" role="tab" aria-selected="'+(settingsTab==="public")+'">公网</button></div>'+
      (settingsTab==="permissions"?
       '<label class="settings-row"><span>默认分享会话<small>当前与新接入的 Agent 沿用已有默认规则</small></span><input type="checkbox" class="settings-switch" '+(defaults.share?"checked":"")+' data-default-share></label>'+
       '<label class="settings-row"><span>默认允许网页操作<small>Agent 模式下，电脑与网页可同时操作。</small></span><input type="checkbox" class="settings-switch" '+(defaults.control?"checked":"")+' '+(!defaults.share?"disabled":"")+' data-default-control></label>'+
       '<div class="settings-section-heading"><span>当前会话</span><span class="access-session-tag">'+rows.length+' 个已接入</span></div>'+
       rows.map(item=>'<div class="access-session"><span class="status-dot" data-state="'+item.state+'"></span><div class="access-session-info"><strong>'+esc(item.title)+'</strong><small>Agent 已接入 · '+esc(item.cwd)+'</small></div><select data-share-session="'+item.id+'" aria-label="'+esc(item.title)+' 分享权限"><option value="default" '+(item.shareRule==="default"?"selected":"")+'>跟随默认 · '+defaultLabel+'</option><option value="control" '+(item.shareRule==="control"?"selected":"")+'>允许操作</option><option value="readonly" '+(item.shareRule==="readonly"?"selected":"")+'>仅查看</option><option value="off" '+(item.shareRule==="off"?"selected":"")+'>不分享</option></select></div>').join("")+
       (rows.length?"":'<p class="terminal-demo-note">暂无活跃 Agent。照常在终端启动后，会话会自动出现在这里。</p>')+
       '<p class="settings-note">'+icon("monitor")+'Agent 分享复用已有权限入口。普通终端仍按原来的接管方式操作。</p>':
       settingsTab==="connection"?'<p class="settings-page-subtitle">当前已连接 · 1 个浏览器在线</p><div class="settings-connection-address">https://sterm.example.invalid</div><button class="settings-integration-btn" data-desktop-copy>复制访问地址</button><p class="terminal-demo-note">沿用已有二维码、访问码和设备管理。手机连接后，可切换「终端 / Agent」。</p>':
       '<p class="settings-page-subtitle">沿用现有 Cloudflare Tunnel / ngrok 配置。</p><p class="terminal-demo-note">Agent 与终端共用同一个网页入口，无需为每个会话配置公网端口。</p><button class="settings-integration-btn" data-desktop-copy>查看现有连接</button>');
  }
  function openSettings(){settingsPage="remote";$("desktop-settings-overlay").hidden=false;renderSettings();$("close-desktop-settings").focus();}
  document.addEventListener("click",event=>{
    const button=event.target.closest("button");if(!button)return;
    if(button.dataset.open){openSheet(button.dataset.open);return;}
    if(button.hasAttribute("data-close-sheet")){closeSheet();return;}
    if(button.dataset.selectSession){send({type:"select",id:button.dataset.selectSession});closeSheet();return;}
    if(button.dataset.model){send({type:"model",value:button.dataset.model});closeSheet();toast("模型设置已更新");return;}
    if(button.dataset.policy){send({type:"policy",value:button.dataset.policy});closeSheet();toast("Agent 权限已更新");return;}
    if(button.dataset.approval){if(!desktop&&!writable(current()))return;send({type:"approval",allow:button.dataset.approval==="allow"});closeSheet();toast(button.dataset.approval==="allow"?"本次操作已批准":"本次操作已拒绝");return;}
    if(button.hasAttribute("data-copy-message")||button.hasAttribute("data-copy-link")||button.hasAttribute("data-desktop-copy")){send({type:"copy"});toast("已演示复制");return;}
    if(button.hasAttribute("data-message-detail")){openSheet("changes");return;}
    if(button.hasAttribute("data-reconnect")||button.id==="retry-connection"){send({type:"reconnect"});closeSheet();toast("连接已恢复");return;}
    if(button.hasAttribute("data-end-confirm")){send({type:"end"});closeSheet();return;}
    if(button.dataset.font){fontSize=Math.max(11,Math.min(19,fontSize+Number(button.dataset.font)));$("messages").style.fontSize=fontSize+"px";fillSheet("display");return;}
    if(button.dataset.mode){setMode(button.dataset.mode);return;}
    if(button.id==="back-to-agent"||button.id==="return-agent-inline"){setMode("agent");return;}
    if(button.id==="stop-turn"||button.hasAttribute("data-native-stop")){send({type:"stop"});toast("本轮已停止，会话保留");return;}
    if(button.id==="show-queue"){openSheet("queue");return;}
    if(button.id==="dismiss-error"){send({type:"dismissError"});return;}
    if(["open-desktop-settings","desktop-settings-link","desktop-share-settings"].includes(button.id)){openSettings();return;}
    if(button.id==="close-desktop-settings"){$("desktop-settings-overlay").hidden=true;return;}
    if(button.dataset.settingsPage){settingsPage=button.dataset.settingsPage;for(const b of document.querySelectorAll("[data-settings-page]"))b.classList.toggle("active",b===button);renderSettings();return;}
    if(button.dataset.settingsTab){settingsTab=button.dataset.settingsTab;renderSettings();return;}
    if(["desktop-add","desktop-new-shell"].includes(button.id)){send({type:"launch"});return;}
    if(button.id==="desktop-history"){send({type:"select",id:"api"});toast("从原有历史列表恢复 · 无需填写 ID");return;}
  });
  document.addEventListener("change",event=>{
    const target=event.target;
    if(target.dataset.shareSession)send({type:"share",id:target.dataset.shareSession,value:target.value});
    else if(target.hasAttribute("data-default-control"))send({type:"defaults",key:"control",value:target.checked});
    else if(target.hasAttribute("data-default-share"))send({type:"defaults",key:"share",value:target.checked});
    else if(target.hasAttribute("data-remote-toggle"))toast("原型：沿用已有远程总开关");
    else if(target.hasAttribute("data-integration"))toast("原型：自动接入设置已切换");
  });
  $("sheet-layer").querySelector(".sheet-scrim").onclick=()=>closeSheet();
  $("native-form").onsubmit=event=>{
    event.preventDefault();const text=$("native-input").value.trim();
    if(!text||isEmpty()||current()?.state==="ended")return;
    send({type:"send",text});$("native-input").value="";
  };
  $("native-input").onkeydown=event=>{
    if(event.key==="Escape"&&busy(current())){event.preventDefault();send({type:"stop"});}
  };
  $("composer-form").onsubmit=event=>{
    event.preventDefault();const row=current();if(!writable(row)||!$("draft").value.trim())return;
    const text=$("draft").value;$("draft").value="";localDrafts.set(row.id,"");send({type:"send",text});render();
  };
  $("draft").oninput=()=>{localDrafts.set(data.active,$("draft").value);$("send-button").disabled=!writable(current())||!$("draft").value.trim();};
  $("draft").addEventListener("compositionstart",()=>keyboardComposing=true);
  $("draft").addEventListener("compositionend",()=>keyboardComposing=false);
  $("draft").onkeydown=event=>{if(event.key==="Enter"&&(event.ctrlKey||event.metaKey)&&!event.isComposing&&!keyboardComposing){event.preventDefault();$("composer-form").requestSubmit();}};
  document.addEventListener("keydown",event=>{
    if(event.key==="Escape"){if(sheetType){event.preventDefault();closeSheet();}else $("desktop-settings-overlay").hidden=true;}
    if(event.key==="Tab"&&sheetType){
      const nodes=[...$("sheet").querySelectorAll('button:not(:disabled),select:not(:disabled),input:not(:disabled),[tabindex="0"]')];
      if(!nodes.length)return;const first=nodes[0],last=nodes.at(-1);
      if(event.shiftKey&&document.activeElement===first){event.preventDefault();last.focus();}
      else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first.focus();}
    }
  });
  window.addEventListener("message",event=>{
    if(event.source!==parent||!event.data?.stPrototype||event.data.type!=="state")return;
    data=event.data.value;render();
  });
  if(parent!==window)parent.postMessage({stPrototype:true,type:"ready"},"*");
})();
