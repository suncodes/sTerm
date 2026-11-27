import "./style.css";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { RemoteLogin, pairRemoteBrowser } from "../remote/login";
import { applyChanges } from "./state";
import type { AgentChange, AgentAction, AgentItem, AgentRoster, AgentSession } from "./types";

const byId = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const list=byId("agent-sessions"), messages=byId("agent-messages"), approvals=byId("agent-approvals"), conversation=byId("conversation");
const draft=byId<HTMLTextAreaElement>("agent-draft"), model=byId<HTMLSelectElement>("agent-model");
const notice=byId("agent-notice"), connection=byId("agent-connection");
const snapshots=new Map<string,AgentSession>(), drafts=new Map<string,string>(), itemNodes=new Map<string,{node:HTMLElement;text:string}>();
const pendingApprovals=new Set<string>(), submitting=new Set<string>(), queues=new Map<string,string[]>();
const storedSession="sterm-agent-selection";
let roster:AgentSession[]=[],current="",authenticated=false,polling=false,connected=false,wasConnected=false;
let sheetType="",sheetRequest="",sheetKey="",beforeSheet:HTMLElement|null=null,toastTimer:number,displaySize=13;
try{current=sessionStorage.getItem(storedSession)??"";}catch{/* 隐私模式使用内存。 */}
const labels:Record<string,string>={connecting:"连接中",idle:"空闲",running:"运行中",waiting:"等待审批",offline:"Agent 已断开",exited:"已结束"};
const login=new RemoteLogin(byId("login-panel"),()=>void authenticate());
const esc=(value:unknown):string=>String(value??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]!));
const icon=(name:string):string=>'<svg class="icon" aria-hidden="true"><use href="#i-'+name+'"/></svg>';
const active=():AgentSession|undefined=>snapshots.get(current);
const busy=(row=active()):boolean=>row?.state==="running"||row?.state==="waiting";
function writable(row=active()):boolean{return Boolean(connected&&row?.allowControl&&!["offline","connecting","exited"].includes(row.state));}
function policyLabel(row:AgentSession):string{
  if(row.agent==="claude")return row.policy==="plan"?"Plan":row.policy==="acceptEdits"?"允许编辑":row.policy||"由 Claude 管理";
  return row.policy.startsWith("workspaceWrite")?"工作区写入":row.policy.startsWith("readOnly")?"只读":row.policy||"由 Codex 管理";
}
function showNotice(text=""):void{byId("notice-text").textContent=text;notice.hidden=!text;}
function toast(text:string):void{byId("toast").textContent=text;byId("toast").hidden=false;clearTimeout(toastTimer);toastTimer=window.setTimeout(()=>byId("toast").hidden=true,2300);}
function saveDraft(id:string,text:string):void{if(!id)return;drafts.set(id,text);try{sessionStorage.setItem("sterm-agent-draft-"+id,text);}catch{/* 配额不足时保留内存。 */}}
function restoreDraft(id:string):string{try{return drafts.get(id)??sessionStorage.getItem("sterm-agent-draft-"+id)??"";}catch{return drafts.get(id)??"";}}
async function request<T>(path:string,body?:unknown):Promise<T>{
  const response=await fetch(path,{signal:AbortSignal.timeout(8000),method:"POST",headers:{"content-type":"application/json"},body:body===undefined?undefined:JSON.stringify(body),credentials:"same-origin",cache:"no-store"});
  if(response.status===401){authenticated=false;connected=false;byId("agent-layout").hidden=true;closeSheet();void login.check();throw new Error("访问授权已失效，请重新登录");}
  if(!response.ok){
    const detail=await response.json().catch(()=>({})) as {message?:string};
    throw new Error(detail.message||(response.status===404?"会话已结束或停止分享":response.status===409?"操作未被接受，请核对会话状态与分享权限":"连接失败，请稍后重试"));
  }
  return response.json() as Promise<T>;
}
function syncControls():void{
  const row=active(),write=writable(),available=write&&Boolean(row?.capabilities.send),sending=submitting.has(current);
  draft.disabled=!available||sending;
  byId<HTMLButtonElement>("agent-send").disabled=!available||sending||!draft.value.trim();
  byId("agent-send").textContent=sending?"确认中…":busy()?"追加":"发送";
  byId("composer").hidden=!available;
  byId("composer-unavailable").hidden=available;
  byId("composer-unavailable").textContent=!row?"在电脑终端使用 Agent 后，从会话列表选择":!connected?"草稿已保留，恢复连接后可继续发送":row.state==="exited"?"会话已结束，内容已保留":!row.allowControl?"当前为只读，电脑端可修改分享权限":row.state==="offline"?"Agent 已断开，等待电脑端恢复":"正在连接 Agent…";
  byId("agent-input-hint").textContent=busy()?"由当前 Agent 安排执行":"电脑端与网页均可操作";
  draft.placeholder=busy()?"补充要求，追加到当前会话…":"继续这个会话…";
  const stop=byId<HTMLButtonElement>("agent-stop");
  stop.hidden=!busy()||!row?.capabilities.stop||!row.allowControl;
  stop.disabled=!write||!row?.turnId;
  byId<HTMLButtonElement>("model-button").disabled=!write||!row?.capabilities.models||row.state!=="idle";
  byId<HTMLButtonElement>("policy-button").disabled=!write||!(row?.capabilities.policy||row?.capabilities.plan)||row.state!=="idle";
  model.disabled=byId<HTMLButtonElement>("model-button").disabled;
  for(const button of approvals.querySelectorAll<HTMLButtonElement>("[data-decision]"))button.disabled=!write||!row?.capabilities.approvals||pendingApprovals.has(current+":"+button.closest<HTMLElement>("[data-request]")?.dataset.request);
  byId("connection-banner").hidden=connected;
  const queued=queues.get(current)??[];
  byId("queue-bar").hidden=!queued.length||!row;
  byId("queue-text").textContent="已接受 "+queued.length+" 条追加消息，执行进度以 Agent 输出为准";
  if(sheetType==="end-confirm"&&!write)closeSheet();
  if((sheetType==="models"||sheetType==="policy")&&(!write||busy()))closeSheet();
}
async function command(action:AgentAction,row=active()):Promise<void>{
  if(!row||!writable(row))throw new Error("当前会话不可操作");
  const identity=row.runtimeId,generation=row.generation;
  const id=Array.from(crypto.getRandomValues(new Uint8Array(16)),b=>b.toString(16).padStart(2,"0")).join("");
  try{
    const result=await request<{id:string;status:string}>("/api/agents/command",{id,runtimeId:identity,generation,sessionId:row.sessionId,settingsRevision:row.settingsRevision,...action});
    if(identity===current)showNotice(result.status==="pending"?"正在确认操作…":"");
    for(let attempt=0;attempt<60;attempt++){
      await new Promise(resolve=>setTimeout(resolve,200));
      const receipt=await request<{status:string;message?:string}>("/api/agents/"+identity+"/commands/"+id);
      if(receipt.status!=="pending"&&receipt.status!=="dispatching"){
        if(receipt.status==="rejected")throw new Error(receipt.message||"Agent 拒绝了此操作");
        if(identity===current)showNotice();
        void poll();return;
      }
    }
    throw new Error("操作结果尚未确认，请核对会话后再操作；不会自动重发");
  }catch(error){if(identity===current)showNotice(error instanceof Error?error.message:String(error));throw error;}
}
function renderRoster(rows:AgentSession[]):void{
  const key=JSON.stringify([current,rows.map(r=>[r.runtimeId,r.title,r.state,r.allowControl])]);
  if(list.dataset.key===key)return;list.dataset.key=key;list.replaceChildren();
  byId("session-count").textContent=String(rows.length);byId("agent-empty").hidden=rows.length>0;
  for(const row of rows){
    const button=document.createElement("button");button.className="agent-session";button.dataset.runtime=row.runtimeId;button.setAttribute("role","tab");button.setAttribute("aria-selected",String(current===row.runtimeId));button.setAttribute("aria-current",String(current===row.runtimeId));
    const dot=document.createElement("span");dot.className="status-dot";dot.dataset.state=row.state;
    const name=document.createElement("strong");name.textContent=row.title;
    const status=document.createElement("small");status.className="tab-status";status.textContent=labels[row.state]??row.state;
    button.append(dot,name,status);button.onclick=()=>select(row.runtimeId);list.append(button);
  }
}
function select(id:string):void{
  if(current)saveDraft(current,draft.value);
  current=id;try{sessionStorage.setItem(storedSession,id);}catch{/* 使用内存选择。 */}
  draft.value=restoreDraft(id);itemNodes.clear();messages.replaceChildren();approvals.replaceChildren();approvals.dataset.key="";showNotice();closeSheet();
  renderRoster(roster);const row=active();if(row)render(row);
  else{byId("agent-title").textContent=id?"正在连接…":"等待分享会话";byId("agent-path").textContent="";byId("agent-state").textContent="";byId("share-access").textContent="";byId("agent-settings").hidden=true;byId("activity-bar").hidden=true;}
  syncControls();
}
function renderItem(item:AgentItem,row:AgentSession):HTMLElement{
  const detail=item.kind==="tool"||item.kind==="thinking";
  const card=document.createElement(detail?"details":"article");card.className="agent-item agent-item-"+item.kind;card.dataset.item=item.id;
  const label=document.createElement(detail?"summary":"div");label.className="agent-item-label";
  label.textContent=item.kind==="user"?"你":item.kind==="assistant"?(item.title||(row.agent==="claude"?"Claude Code":"Codex")):item.title||(item.kind==="thinking"?"思考摘要":"工具");
  if(item.kind==="assistant"){const avatar=document.createElement("span");avatar.className="agent-avatar";avatar.textContent=row.agent==="claude"?"C":"›_";label.prepend(avatar);}
  if(detail){const status=document.createElement("small");status.className="agent-item-status";status.textContent=["running","inProgress"].includes(item.status)?"执行中":["error","failed"].includes(item.status)?"失败":item.status==="declined"?"已拒绝":item.status==="interrupted"?"已中断":"已完成";label.append(status);}
  const content=document.createElement("div");content.className="agent-content";
  if(item.kind==="tool"||item.kind==="user"){const pre=document.createElement("pre");pre.textContent=item.text;content.append(pre);}
  else{
    content.innerHTML=DOMPurify.sanitize(marked.parse(item.text,{async:false}) as string,{FORBID_TAGS:["img","iframe","style","form"],FORBID_ATTR:["style"]});
    for(const link of content.querySelectorAll("a")){link.target="_blank";link.rel="noopener noreferrer";}
  }
  if(item.kind==="assistant"&&["running","inProgress"].includes(item.status))content.classList.add("message-running");
  card.append(label,content);return card;
}
function approvalButtons(id:string,disabled:boolean):string{
  const pending=pendingApprovals.has(current+":"+id);
  return '<button data-decision="false" '+(disabled||pending?"disabled":"")+'>拒绝</button><button class="approve" data-decision="true" '+(disabled||pending?"disabled":"")+'>'+(pending?"确认中…":"批准一次")+'</button>';
}
function renderApprovals(row:AgentSession):void{
  approvals.hidden=!row.approvals.length;
  const key=JSON.stringify([row.approvals,writable(),[...pendingApprovals]]);
  if(approvals.dataset.key===key)return;approvals.dataset.key=key;
  approvals.innerHTML=row.approvals.map(a=>'<section class="approval-card agent-approval" data-request="'+esc(a.id)+'"><div class="approval-heading">'+icon("lock")+'需要你的审批<small>'+esc(a.title)+'</small></div><p>电脑端也可以处理本次操作。</p><pre class="approval-target">'+esc(a.detail)+'</pre><div class="approval-actions"><button class="details-link" data-approval-detail="'+esc(a.id)+'">查看详情</button>'+approvalButtons(a.id,!writable()||!row.capabilities.approvals)+'</div></section>').join("");
}
function render(row:AgentSession):void{
  if(row.runtimeId!==current)return;
  const atBottom=conversation.scrollHeight-conversation.scrollTop-conversation.clientHeight<70;
  byId("agent-title").textContent=row.title;byId("agent-path").textContent=row.cwd;byId("agent-state").textContent=labels[row.state]??row.state;
  byId("agent-state").dataset.state=row.state;byId("current-dot").dataset.state=row.state;
  byId("share-access").textContent=row.allowControl?"双端可操作":"只读";byId("share-access").dataset.readonly=String(!row.allowControl);
  byId("agent-settings").hidden=false;byId("current-model").textContent=row.model||"由 Agent 管理";byId("agent-policy").textContent=policyLabel(row);
  byId("settings-current-state").textContent=labels[row.state]??row.state;
  const options=[...new Set([row.model,...row.models])].filter(Boolean);
  if(model.dataset.key!==options.join("|")){model.replaceChildren(...options.map(value=>new Option(value,value)));model.dataset.key=options.join("|");}model.value=row.model;
  const present=new Set(row.items.map(i=>i.id));
  for(const [id,old] of itemNodes)if(!present.has(id)){old.node.remove();itemNodes.delete(id);}
  for(const item of row.items){
    const signature=JSON.stringify(item),old=itemNodes.get(item.id);if(old?.text===signature)continue;
    const node=renderItem(item,row);
    if(old){if(node instanceof HTMLDetailsElement&&old.node instanceof HTMLDetailsElement)node.open=old.node.open;old.node.replaceWith(node);}else messages.append(node);
    itemNodes.set(item.id,{node,text:signature});
  }
  let position=messages.firstElementChild;
  for(const item of row.items){const node=itemNodes.get(item.id)?.node;if(!node)continue;if(node!==position)messages.insertBefore(node,position);position=node.nextElementSibling;}
  renderApprovals(row);
  byId("activity-bar").hidden=false;byId("activity-dot").dataset.state=row.state;
  byId("activity-text").textContent=row.state==="running"?"正在生成回复…":row.state==="waiting"?"等待审批 · 电脑端也可以处理":row.state==="idle"?"本轮已完成":labels[row.state]??row.state;
  if(row.state==="idle"||row.state==="exited")queues.delete(row.runtimeId);
  if(row.error)showNotice(row.error);
  if(sheetType==="approval-detail"&&!row.approvals.some(a=>a.id===sheetRequest))closeSheet();
  syncControls();updateSheet();
  if(atBottom)conversation.scrollTop=conversation.scrollHeight;
}
function heading(title:string):string{return '<div class="sheet-heading"><h2 id="sheet-title">'+title+'</h2><button class="touch-button" data-close-sheet aria-label="关闭面板">'+icon("close")+'</button></div>';}
function fillSheet():void{
  const row=active();let html="";
  if(sheetType==="sessions")html=heading("会话")+'<p class="sheet-description">电脑端已接入且获准分享的 Agent 会话。</p><div class="sheet-group-label">工作电脑 · '+roster.length+' 个共享会话</div>'+roster.map(r=>'<button class="sheet-list-button" data-select="'+esc(r.runtimeId)+'"><span><strong>'+esc(r.title)+'</strong><small>'+esc(r.cwd)+'</small><small>'+esc(labels[r.state]??r.state)+' · '+(r.allowControl?"允许操作":"只读")+'</small></span>'+(current===r.runtimeId?icon("check"):"")+'</button>').join("")+'<p class="sheet-footnote">切换只改变当前网页，其他终端继续使用各自的会话。</p>';
  else if(sheetType==="models"&&row)html=heading("切换模型")+'<p class="sheet-description">当前 Agent 提供的模型，选择后用于后续请求。</p><div role="radiogroup" aria-label="可用模型">'+[...new Set([row.model,...row.models])].filter(Boolean).map(value=>'<button class="sheet-list-button" role="radio" aria-checked="'+(row.model===value)+'" data-model="'+esc(value)+'"><span><strong>'+esc(value)+'</strong></span>'+(row.model===value?icon("check"):"")+'</button>').join("")+'</div>';
  else if(sheetType==="policy"&&row)html=heading("Agent 权限")+'<p class="sheet-description">当前执行范围：'+esc(policyLabel(row))+'。网页是否允许输入由电脑端分享设置决定。</p>'+(row.capabilities.policy?'<button class="sheet-list-button" data-sandbox="read-only"><span><strong>只读</strong><small>工具执行按需审批</small></span></button><button class="sheet-list-button" data-sandbox="workspace-write"><span><strong>工作区写入</strong><small>允许工作区修改，额外权限按需审批</small></span></button>':row.capabilities.plan?'<button id="agent-plan" class="sheet-list-button" data-plan><span><strong>进入 Plan</strong><small>先分析和规划，其他权限模式由原生 Claude 管理</small></span></button>':'<p class="sheet-description">当前 Agent 未提供远程权限切换。</p>');
  else if(sheetType==="approval-detail"&&row){const a=row.approvals.find(a=>a.id===sheetRequest);if(!a){closeSheet();return;}html=heading("审批详情")+'<p class="sheet-description">'+esc(a.title)+'</p><pre class="detail-code">'+esc(a.detail)+'</pre><p class="sheet-description">批准仅针对这次操作。任一侧处理后，其余页面同步关闭待办。</p><div class="approval-actions" data-request="'+esc(a.id)+'">'+approvalButtons(a.id,!writable()||!row.capabilities.approvals)+'</div>';}
  else if(sheetType==="queue")html=heading("已追加的消息")+'<p class="sheet-description">Agent 已接受以下消息，是否排队或立即处理由 Agent 决定。</p>'+(queues.get(current)??[]).map((text,i)=>'<div class="detail-code">'+(i+1)+'. '+esc(text)+'</div>').join("")+'<p class="sheet-footnote">停止本轮不保证清空原生队列。</p>';
  else if(sheetType==="more")html=heading("会话选项")+'<button class="menu-item" data-open="sessions">'+icon("list")+'所有会话</button><button class="menu-item" data-open="info">'+icon("monitor")+'会话信息</button><button class="menu-item" data-open="display">'+icon("settings")+'显示设置</button><button class="menu-item" data-copy-link>'+icon("copy")+'复制会话链接</button><button class="menu-item" data-reconnect>'+icon("refresh")+'重新连接</button>'+(row?.capabilities.end?'<button class="menu-item danger" data-open="end-confirm" '+(!writable()?"disabled":"")+'>'+icon("stop")+'结束 Agent 会话</button>':"");
  else if(sheetType==="end-confirm"&&row)html=heading("结束 Agent 会话？")+'<p class="sheet-description">将结束电脑端的「'+esc(row.title)+'」。网页保留已显示的内容。仅中断当前回复请使用「停止生成」。</p><div class="confirm-actions"><button class="subtle-button" data-close-sheet>取消</button><button class="danger-button" data-end>结束会话</button></div>';
  else if(sheetType==="info")html=heading("会话信息")+(row?'<pre class="detail-code">'+esc(row.title)+'\n'+esc(row.cwd)+'\n'+esc(row.agent)+' · '+esc(row.model)+'\n执行权限：'+esc(policyLabel(row))+'\n网页：'+(row.allowControl?"允许操作":"只读")+'</pre>':'<p class="sheet-description">暂无共享会话。</p>');
  else if(sheetType==="display")html=heading("显示设置")+'<p class="sheet-description">只改变网页消息，不影响电脑终端。</p><div class="menu-item">消息字号<span class="menu-value"><button class="subtle-button" data-font="-1" aria-label="减小字号">−</button> '+displaySize+' px <button class="subtle-button" data-font="1" aria-label="增大字号">+</button></span></div>';
  byId("sheet").innerHTML=html;
}
function updateSheet():void{
  if(!sheetType)return;
  const row=active(),key=JSON.stringify([sheetType,sheetRequest,roster.map(r=>[r.runtimeId,r.title,r.state,r.allowControl]),row?.approvals,row?.allowControl,row?.model,row?.policy,[...pendingApprovals]]);
  if(sheetKey===key)return;sheetKey=key;
  const focused=document.activeElement as HTMLElement|null,keyAttr=focused?.dataset.select,decision=focused?.dataset.decision;
  fillSheet();if(keyAttr)byId("sheet").querySelector<HTMLButtonElement>('[data-select="'+CSS.escape(keyAttr)+'"]')?.focus();
  else if(decision!==undefined)byId("sheet").querySelector<HTMLButtonElement>('[data-decision="'+decision+'"]')?.focus();
}
function openSheet(type:string):void{
  if(!sheetType)beforeSheet=document.activeElement as HTMLElement;
  sheetType=type;sheetKey="";byId("sheet-layer").dataset.sheet=type;byId("sheet-layer").hidden=false;fillSheet();byId("sheet").querySelector<HTMLButtonElement>("button")?.focus();
}
function closeSheet():void{byId("sheet-layer").hidden=true;sheetType="";sheetKey="";if(beforeSheet?.isConnected)beforeSheet.focus();}
async function poll():Promise<void>{
  if(polling||!authenticated)return;polling=true;
  try{
    const data=await request<AgentRoster>("/api/agents");roster=data.sessions;
    const alive=new Set(roster.map(r=>r.runtimeId));
    for(const id of snapshots.keys())if(!alive.has(id))snapshots.delete(id);
    connected=true;
    if(!current||!alive.has(current)){if(roster.length||current)select(roster[0]?.runtimeId??"");}
    renderRoster(roster);updateSheet();
    const id=current;
    if(id){
      const seq=snapshots.get(id)?.seq;
      const result=await request<{unchanged?:boolean;snapshot?:AgentSession;changes?:AgentChange[]}>("/api/agents/"+id+(seq===undefined?"":"?seq="+seq));
      let next=result.snapshot;
      if(result.changes){const old=snapshots.get(id);next=old?applyChanges(old,result.changes)??undefined:undefined;if(!next)next=(await request<{snapshot:AgentSession}>("/api/agents/"+id)).snapshot;}
      if(next){if(next.protocolVersion!==1)throw new Error("Agent 协议版本不兼容，请更新 sTerm");snapshots.set(id,next);if(id===current)render(next);}
    }
    if(!wasConnected){showNotice();wasConnected=true;}
    connection.textContent="已连接";connection.dataset.state="connected";syncControls();
  }catch(error){
    connected=false;wasConnected=false;connection.textContent="重连中";connection.dataset.state="offline";
    if(authenticated)showNotice(error instanceof Error?error.message:String(error));syncControls();
  }finally{polling=false;}
}
async function authenticate():Promise<void>{
  try{const response=await fetch("/api/auth",{credentials:"same-origin",cache:"no-store"}),status=await response.json() as {authenticated:boolean};if(!response.ok||!status.authenticated){void login.check();return;}login.hide();authenticated=true;byId("agent-layout").hidden=false;await poll();}catch{void login.check();}
}
document.addEventListener("click",event=>{
  const button=(event.target as Element).closest<HTMLButtonElement>("button");if(!button||button.disabled)return;
  if(button.dataset.open){openSheet(button.dataset.open);return;}
  if(button.hasAttribute("data-close-sheet")){closeSheet();return;}
  if(button.dataset.select){select(button.dataset.select);void poll();return;}
  if(button.dataset.model){const value=button.dataset.model;closeSheet();void command({action:"setModel",model:value}).catch(()=>{});return;}
  if(button.dataset.sandbox){const value=button.dataset.sandbox;closeSheet();void command({action:"setPolicy",sandbox:value,approvalPolicy:"on-request"}).catch(()=>{});return;}
  if(button.hasAttribute("data-plan")){closeSheet();void command({action:"enterPlan"}).catch(()=>{});return;}
  if(button.dataset.approvalDetail){sheetRequest=button.dataset.approvalDetail;openSheet("approval-detail");return;}
  if(button.dataset.decision!==undefined){
    const row=active(),requestId=button.closest<HTMLElement>("[data-request]")?.dataset.request;if(!row||!requestId||!writable())return;
    const key=row.runtimeId+":"+requestId;if(pendingApprovals.has(key))return;pendingApprovals.add(key);syncControls();updateSheet();
    void command({action:"respondApproval",requestId,allow:button.dataset.decision==="true"},row).catch(()=>{}).finally(()=>{pendingApprovals.delete(key);syncControls();updateSheet();});return;
  }
  if(button.hasAttribute("data-reconnect")){closeSheet();void poll();return;}
  if(button.hasAttribute("data-end")){
    const row=active();if(!row||!writable()||!row.capabilities.end)return;button.disabled=true;
    void request("/api/agents/"+row.runtimeId+"/end",{generation:row.generation,sessionId:row.sessionId}).then(()=>{closeSheet();void poll();}).catch(error=>{showNotice(error.message);button.disabled=false;});return;
  }
  if(button.hasAttribute("data-copy-link")){
    const url=new URL(location.href);url.search="";url.hash="";
    if(current)url.searchParams.set("session",current);
    if(!navigator.clipboard){toast("当前浏览器不允许复制，可复制地址栏");return;}
    void navigator.clipboard.writeText(url.href).then(()=>toast("会话链接已复制")).catch(()=>toast("当前浏览器不允许复制，可复制地址栏"));return;
  }
  if(button.dataset.font){displaySize=Math.max(11,Math.min(19,displaySize+Number(button.dataset.font)));messages.style.fontSize=displaySize+"px";fillSheet();}
});
draft.oninput=()=>{saveDraft(current,draft.value);syncControls();};
byId<HTMLFormElement>("agent-composer").onsubmit=event=>{
  event.preventDefault();const row=active();if(!row||!draft.value.trim()||submitting.has(current)||!writable()||!row.capabilities.send)return;
  const text=draft.value,id=current,queued=busy(row);submitting.add(id);syncControls();
  void command({action:"sendText",text},row).then(()=>{
    if(queued){queues.set(id,[...(queues.get(id)??[]),text]);}
    if(drafts.get(id)===text){drafts.delete(id);try{sessionStorage.removeItem("sterm-agent-draft-"+id);}catch{/* 内存状态已更新。 */}}
    if(current===id&&draft.value===text)draft.value="";
  }).catch(()=>{}).finally(()=>{submitting.delete(id);syncControls();});
};
draft.onkeydown=event=>{if(event.key==="Enter"&&(event.ctrlKey||event.metaKey)&&!event.isComposing){event.preventDefault();byId<HTMLFormElement>("agent-composer").requestSubmit();}};
byId("agent-stop").onclick=()=>{const row=active();if(row?.turnId)void command({action:"stopTurn",turnId:row.turnId},row).catch(()=>{});};
model.onchange=()=>void command({action:"setModel",model:model.value}).catch(()=>{const row=active();if(row)model.value=row.model;});
byId("dismiss-notice").onclick=()=>showNotice();
byId("sheet-layer").querySelector(".sheet-scrim")!.addEventListener("click",()=>closeSheet());
document.addEventListener("keydown",event=>{
  if(event.key==="Escape"&&sheetType){event.preventDefault();closeSheet();}
  if(event.key==="Tab"&&sheetType){const nodes=[...byId("sheet").querySelectorAll<HTMLButtonElement>("button:not(:disabled)")],first=nodes[0],last=nodes[nodes.length-1];if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}
});
function viewport():void{document.documentElement.style.setProperty("--viewport-height",(window.visualViewport?.height??window.innerHeight)+"px");}
window.visualViewport?.addEventListener("resize",viewport);window.addEventListener("resize",viewport);viewport();
const querySession=new URLSearchParams(location.search).get("session");if(querySession)current=querySession;
draft.value=restoreDraft(current);
window.setInterval(()=>void poll(),350);
const pair=new URLSearchParams(location.hash.slice(1)).get("pair");
if(pair)void pairRemoteBrowser(pair,false).then(()=>{history.replaceState(null,"",location.pathname+location.search);void authenticate();}).catch(error=>void login.check(String(error)));
else void authenticate();
