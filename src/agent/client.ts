import "./style.css";
import { RemoteLogin, pairRemoteBrowser } from "../remote/login";
import { applyChanges } from "./state";
import { conversationItems, liveTasks } from "./display";
import { TimelineView } from "./timeline-view";
import { activeNotices, importantNotice, noticeFingerprint, noticeKey, NoticeTracker } from "./notices";
import { blockedHtml, blockReview } from "./blocking";
import { interactionHtml, readInteraction, tasksHtml, taskLabel } from "./interactions";
import { questionnaireView, questionPage, syncQuestionnaire, showQuestion, unansweredQuestion, type QuestionnaireView } from "./questionnaire";
import { runningStates, recordDuration, durationLabel, taskTimingHtml } from "./presentation";
import type { AgentChange, AgentAction, AgentRoster, AgentSession, AgentInteraction, AgentPermission, AgentNotice } from "./types";

const byId = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const list=byId("agent-sessions"), messages=byId("agent-messages"), approvals=byId("agent-approvals"), conversation=byId("conversation");
const draft=byId<HTMLTextAreaElement>("agent-draft"), model=byId<HTMLSelectElement>("agent-model");
const notice=byId("agent-notice"), connection=byId("agent-connection");
const snapshots=new Map<string,AgentSession>(), drafts=new Map<string,string>();
const timelineView=new TimelineView(messages);
let taskTimeline:{key:string;node:HTMLElement;view:TimelineView}|undefined;
const pendingApprovals=new Set<string>(), submitting=new Set<string>(), queues=new Map<string,string[]>();
type InteractionDraft={fields:{key:string;value:string;checked:boolean;selected?:boolean[]}[];questionnaire?:QuestionnaireView};
const interactionDrafts=new Map<string,InteractionDraft>(),expandedComposers=new Set<string>();
const collapsedInteractions=new Set<string>(),interactionErrors=new Map<string,string>(),clockAnchors=new Map<string,{received:number;clock:number}>();
const taskOutputs=new Map<string,{text:string;truncated?:boolean}>();
const pendingBlocks=new Set<string>();
const noticeTracker=new NoticeTracker(),dismissedNotices=new Set<string>(),seenErrors=new Map<string,string|null>();
let noticeTimer:number,transientNotice:{scope:string;value:AgentNotice;until:number}|undefined;
let cardRequest="",cardKey="",composerRequest="";
let permissionChoice:AgentPermission|undefined;
const storedSession="sterm-agent-selection";
let roster:AgentSession[]=[],current="",authenticated=false,polling=false,connected=false,wasConnected=false;
let sheetType="",sheetRequest="",sheetKey="",beforeSheet:HTMLElement|null=null,toastTimer:number,displaySize=13;
try{current=sessionStorage.getItem(storedSession)??"";}catch{/* 隐私模式使用内存。 */}
const labels:Record<string,string>={connecting:"连接中",idle:"空闲",running:"运行中",waiting:"等待回答",retrying:"API 重试中",failed:"本轮失败",interrupted:"已中断",blocked:"安全阻断",offline:"Agent 已断开",exited:"已结束"};
const login=new RemoteLogin(byId("login-panel"),()=>void authenticate());
const esc=(value:unknown):string=>String(value??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]!));
const icon=(name:string):string=>'<svg class="icon" aria-hidden="true"><use href="#i-'+name+'"/></svg>';
const active=():AgentSession|undefined=>snapshots.get(current);
const busy=(row=active()):boolean=>["running","waiting","retrying"].includes(row?.state??"");
function writable(row=active()):boolean{return Boolean(connected&&row?.allowControl&&!["offline","connecting","exited"].includes(row.state));}
function policyLabel(row:AgentSession):string{
  const selected=row.permissions?.options?.find(p=>p.profile===row.permissions?.current?.profile&&p.approvalPolicy===row.permissions.current.approvalPolicy&&p.reviewer===row.permissions.current.reviewer);
  if(selected)return selected.label;
  if(row.permissions?.current?.profile===":danger-full-access")return "Full Access";
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
  const row=active(),write=writable(),available=write&&Boolean(row?.capabilities.send)&&row?.state!=="blocked"&&row?.activity?.kind!=="blocked",sending=submitting.has(current);
  draft.disabled=!available||sending;
  byId<HTMLButtonElement>("agent-send").disabled=!available||sending||!draft.value.trim();
  byId("agent-send").textContent=sending?"确认中…":busy()?"追加":"发送";
  byId("composer").hidden=!available;
  const blocking=row?.interactions?.find(item=>item.blocking&&!item.agentId),composerKey=blocking?current+":"+row?.sessionId+":"+blocking.id:"";
  if(composerKey&&document.activeElement===draft&&draft.value.trim())expandedComposers.add(composerKey);
  composerRequest=composerKey;
  for(const key of expandedComposers)if(![...snapshots.values()].some(session=>(session.interactions??[]).some(item=>key===session.runtimeId+":"+session.sessionId+":"+item.id)))expandedComposers.delete(key);
  const composerCollapsed=Boolean(composerKey&&!expandedComposers.has(composerKey));
  byId("composer").dataset.collapsed=String(composerCollapsed);
  byId("agent-composer").hidden=composerCollapsed;byId("composer-mode").hidden=!composerKey;
  byId("composer-toggle").textContent=composerCollapsed?"补充要求":"收起补充输入";
  byId("composer-toggle").setAttribute("aria-expanded",String(!composerCollapsed));
  byId("composer-draft-hint").textContent=composerCollapsed&&draft.value?"草稿已保留":"";
  byId("composer-unavailable").hidden=available||row?.state==="blocked";
  byId("composer-unavailable").textContent=!row?"在电脑终端使用 Agent 后，从会话列表选择":!connected?"草稿已保留，恢复连接后可继续发送":row.state==="exited"?"会话已结束，内容已保留":!row.allowControl?"当前为只读，电脑端可修改分享权限":row.state==="offline"?"Agent 已断开，等待电脑端恢复":row.state==="blocked"?"请查看安全阻断卡片中的可用操作":"正在连接 Agent…";
  byId("agent-input-hint").textContent=busy()?"由当前 Agent 安排执行":"电脑端与网页均可操作";
  draft.placeholder=busy()?"补充要求，追加到当前会话…":"继续这个会话…";
  const stop=byId<HTMLButtonElement>("agent-stop");
  stop.hidden=!busy()||!row?.capabilities.stop||!row.allowControl;
  stop.disabled=!write||!row?.turnId;
  byId<HTMLButtonElement>("model-button").disabled=!write||!row?.capabilities.models||!["idle","failed","interrupted"].includes(row.state);
  byId<HTMLButtonElement>("policy-button").disabled=!write||!(row?.capabilities.policy||row?.capabilities.plan)||!["idle","failed","interrupted"].includes(row.state);
  model.disabled=byId<HTMLButtonElement>("model-button").disabled;
  for(const button of approvals.querySelectorAll<HTMLButtonElement>("[data-decision]"))button.disabled=!write||!row?.capabilities.approvals||pendingApprovals.has(current+":"+button.closest<HTMLElement>("[data-request]")?.dataset.request);
  byId("connection-banner").hidden=connected;
  const addQueue=byId<HTMLButtonElement>("queue-add");
  addQueue.hidden=!row?.capabilities.queue;addQueue.disabled=!available||sending||!draft.value.trim();
  const queued=row?.capabilities.queue?(row.queuedInputs??[]):queues.get(current)??[];
  byId("queue-bar").hidden=!queued.length||!row;
  byId("queue-text").textContent=row?.capabilities.queue?"原生输入队列："+queued.length+" 条":"已接受 "+queued.length+" 条追加消息，执行进度以 Agent 输出为准";
  for(const button of byId("interaction-dock").querySelectorAll<HTMLButtonElement>("button"))button.disabled=!connected;
  for(const button of byId("interaction-card").querySelectorAll<HTMLButtonElement>("[data-interaction-open],[data-expand-interaction],[data-collapse-interaction]"))button.disabled=!connected;
  const interaction= row?.interactions?.find(item=>item.id===cardRequest),interactionWritable=write&&!(row?.activity?.kind==="blocked"&&!interaction?.agentId);
  for(const field of byId("interaction-card").querySelectorAll<HTMLInputElement|HTMLButtonElement>("input,textarea,select,button[data-interaction-decision],button[data-permission-scope],button[data-form-action],button[data-interaction-cancel],button[type=submit]"))field.disabled=!interactionWritable||pendingApprovals.has(current+":"+cardRequest);
  for(const button of byId("interaction-card").querySelectorAll<HTMLButtonElement>("[data-question-nav]"))button.disabled=pendingApprovals.has(current+":"+cardRequest);
  if(sheetType==="queue")for(const button of byId("sheet").querySelectorAll<HTMLButtonElement>("button[data-queue-operation]")){button.disabled=!write||(button.dataset.queueOperation==="up"&&button.closest(".queued-item")===byId("sheet").querySelector(".queued-item"));}
  if(sheetType==="interaction"){
    for(const field of byId("sheet").querySelectorAll<HTMLInputElement|HTMLButtonElement>("input,textarea,select,button:not([data-close-sheet])"))field.disabled=!write||row?.activity?.kind==="blocked"&&!row.interactions?.find(item=>item.id===sheetRequest)?.agentId||pendingApprovals.has(current+":"+sheetRequest);
  }
  if(sheetType==="interaction")for(const button of byId("sheet").querySelectorAll<HTMLButtonElement>("[data-question-nav]"))button.disabled=pendingApprovals.has(current+":"+sheetRequest);
  const blocked=byId("blocked-card"),review=row?blockReview(row):undefined,ack=blocked.querySelector<HTMLInputElement>("[data-block-acknowledge]"),resume=blocked.querySelector<HTMLButtonElement>("[data-block-continue]");
  const blockPending=Boolean(review&&pendingBlocks.has(review.key));
  if(ack)ack.disabled=!write||blockPending||!review?.canContinue;
  if(resume)resume.disabled=!write||blockPending||!review?.canContinue||!ack?.checked||!blocked.querySelector<HTMLDetailsElement>(".block-findings")?.open;
  if(sheetType==="end-confirm"&&!write)closeSheet();
  if(["models","policy","permission-confirm"].includes(sheetType)&&(!write||!row||!["idle","failed","interrupted"].includes(row.state)))closeSheet();
  scheduleInteractionLayout();
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
  saveCardDraft();cardRequest="";cardKey="";
  const block=byId("blocked-card");block.hidden=true;block.replaceChildren();delete block.dataset.key;delete block.dataset.review;
  closeSheet();current=id;try{sessionStorage.setItem(storedSession,id);}catch{/* 使用内存选择。 */}
  draft.value=restoreDraft(id);timelineView.clear();taskTimeline=undefined;approvals.replaceChildren();approvals.dataset.key="";showNotice();closeSheet();
  renderRoster(roster);const row=active();if(row)render(row);
  else{byId("agent-title").textContent=id?"正在连接…":"等待分享会话";byId("agent-path").textContent="";byId("agent-state").textContent="";byId("share-access").textContent="";byId("agent-settings").hidden=true;byId("activity-bar").hidden=true;}
  syncControls();
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
  timelineView.render(row,conversationItems(row).filter(item=>!item.agentId));
  renderApprovals(row);renderInteractions(row);renderBlocked(row);
  byId("activity-bar").hidden=false;byId("activity-dot").dataset.state=row.state;
  if(row.state==="idle"||row.state==="exited")queues.delete(row.runtimeId);
  const activity=byId("agent-activity-detail");activity.hidden=!busy(row)||!row.activity?.message||row.activity?.kind==="blocked";activity.textContent=row.activity?.message??"";
  activity.dataset.state=row.activity?.kind??row.state;
  const tasks=liveTasks(row),uncertain=tasks.filter(task=>["unknown","blocked","paused"].includes(task.state)).length;
  byId("task-bar").hidden=!tasks.length;
  byId("task-text").textContent=[tasks.length-uncertain?tasks.length-uncertain+" 项任务进行中":"",uncertain?uncertain+" 项状态待确认":""].filter(Boolean).join(" · ");
  renderNotices(row);
  const tokenUsage=row.metadata?.tokenUsage as {total?:{totalTokens?:number};modelContextWindow?:number;last?:{totalTokens?:number}}|undefined;
  const nativeUsage=[...(row.turns??[])].reverse().find(turn=>!turn.agentId)?.usage;
  const usage=byId("usage-summary");usage.hidden=!tokenUsage&&!nativeUsage;usage.textContent=tokenUsage?"Token "+(tokenUsage.total?.totalTokens?.toLocaleString()??"未知")+(tokenUsage.modelContextWindow&&typeof tokenUsage.last?.totalTokens==="number"?" · 上下文 "+Math.round(tokenUsage.last.totalTokens/tokenUsage.modelContextWindow*100)+"%":""):nativeUsage?"本轮 Token · 输入 "+String(nativeUsage.input_tokens??"未知")+" · 输出 "+String(nativeUsage.output_tokens??"未知"):"";
  updateActivity(row);
  const errorScope=JSON.stringify([row.runtimeId,row.sessionId]);
  if(row.error&&seenErrors.get(errorScope)!==row.error)showNotice(row.error);
  seenErrors.set(errorScope,row.error);
  if(sheetType==="approval-detail"&&!row.approvals.some(a=>a.id===sheetRequest))closeSheet();
  syncControls();updateSheet();
  if(atBottom)conversation.scrollTop=conversation.scrollHeight;
}
function renderNotices(row:AgentSession):void{
  const scope=JSON.stringify([row.runtimeId,row.sessionId]),fresh=noticeTracker.update(row);
  const recent=fresh.filter(entry=>entry.phase!=="running"&&!entry.id.startsWith("interaction:"));
  const important=recent.filter(importantNotice).slice(-1)[0];
  if(important)transientNotice={scope,value:important,until:Date.now()+10000};
  else if(recent.length)toast(String(recent[recent.length-1].message||recent[recent.length-1].title||"运行状态已更新").slice(0,240));
  const identity=(entry:AgentNotice):string=>scope+noticeKey(entry)+noticeFingerprint(entry);
  const progress=activeNotices(row).filter(entry=>!dismissedNotices.has(identity(entry)));
  const transient=transientNotice?.scope===scope&&transientNotice.until>Date.now()&&row.notices?.some(entry=>noticeKey(entry)===noticeKey(transientNotice!.value)&&noticeFingerprint(entry)===noticeFingerprint(transientNotice!.value))?transientNotice.value:undefined;
  const shown=transient&&!dismissedNotices.has(identity(transient))?transient:progress[progress.length-1];
  const banner=byId("native-notice");banner.hidden=!shown;banner.dataset.noticeKey=shown?identity(shown):"";
  byId("native-notice-text").textContent=shown?String(shown.message||shown.title||"运行提示")+(shown===progress[progress.length-1]&&progress.length>1?" · 另有 "+(progress.length-1)+" 项进行中":""):"";
  clearTimeout(noticeTimer);
  if(transientNotice?.scope===scope&&transientNotice.until>Date.now())noticeTimer=window.setTimeout(()=>{const currentRow=active();if(currentRow)renderNotices(currentRow);},transientNotice.until-Date.now()+20);
}
function renderBlocked(row:AgentSession):void{
  const card=byId("blocked-card");card.hidden=row.state!=="blocked"&&row.activity?.kind!=="blocked";
  if(card.hidden){card.replaceChildren();delete card.dataset.key;delete card.dataset.review;return;}
  const review=blockReview(row),pending=pendingBlocks.has(review.key),key=JSON.stringify([review.key,row.state,row.activity,writable(),pending]);
  if(card.dataset.key===key)return;
  const same=card.dataset.review===review.key,open=same&&Boolean(card.querySelector<HTMLDetailsElement>(".block-findings")?.open),ack=same&&Boolean(card.querySelector<HTMLInputElement>("[data-block-acknowledge]")?.checked);
  const focused=document.activeElement as HTMLElement|null,wasFocused=Boolean(focused&&card.contains(focused)),control=focused?.hasAttribute("data-block-acknowledge")?"[data-block-acknowledge]":focused?.hasAttribute("data-block-continue")?"[data-block-continue]":"summary";
  const scroll=same?card.querySelector(".interaction-card-body")?.scrollTop??0:0;
  card.dataset.key=key;card.dataset.review=review.key;card.innerHTML=blockedHtml(row,writable(),pending);
  const findings=card.querySelector<HTMLDetailsElement>(".block-findings");if(findings){findings.open=open;findings.ontoggle=()=>syncControls();}
  const checkbox=card.querySelector<HTMLInputElement>("[data-block-acknowledge]");if(checkbox)checkbox.checked=ack;
  if(same&&wasFocused)card.querySelector<HTMLElement>(control)?.focus({preventScroll:true});
  const body=card.querySelector(".interaction-card-body");if(body)body.scrollTop=scroll;
}
function heading(title:string):string{return '<div class="sheet-heading"><h2 id="sheet-title">'+title+'</h2><button class="touch-button" data-close-sheet aria-label="关闭面板">'+icon("close")+'</button></div>';}
function fillSheet():void{
  const row=active();let html="";
  if(sheetType==="sessions")html=heading("会话")+'<p class="sheet-description">电脑端已接入且获准分享的 Agent 会话。</p><div class="sheet-group-label">工作电脑 · '+roster.length+' 个共享会话</div>'+roster.map(r=>'<button class="sheet-list-button" data-select="'+esc(r.runtimeId)+'"><span><strong>'+esc(r.title)+'</strong><small>'+esc(r.cwd)+'</small><small>'+esc(labels[r.state]??r.state)+' · '+(r.allowControl?"允许操作":"只读")+'</small></span>'+(current===r.runtimeId?icon("check"):"")+'</button>').join("")+'<p class="sheet-footnote">切换只改变当前网页，其他终端继续使用各自的会话。</p>';
  else if(sheetType==="models"&&row)html=heading("切换模型")+'<p class="sheet-description">当前 Agent 提供的模型，选择后用于后续请求。</p><div role="radiogroup" aria-label="可用模型">'+[...new Set([row.model,...row.models])].filter(Boolean).map(value=>'<button class="sheet-list-button" role="radio" aria-checked="'+(row.model===value)+'" data-model="'+esc(value)+'"><span><strong>'+esc(value)+'</strong></span>'+(row.model===value?icon("check"):"")+'</button>').join("")+'</div>';
  else if(sheetType==="policy"&&row)html=heading("Agent 权限")+'<p class="sheet-description">当前执行范围：'+esc(policyLabel(row))+'。设置来自当前 CLI 的原生权限目录。</p>'+(row.capabilities.policy?(row.permissions?.options??[]).map((p,index)=>'<button class="sheet-list-button" data-permission="'+index+'" '+(!p.allowed?'disabled':"")+'><span><strong>'+esc(p.label)+'</strong><small>'+esc(p.description||p.profile)+' · '+esc(p.approvalPolicy)+' · '+esc(p.reviewer)+(p.allowed?"":" · 被原生策略限制")+'</small></span></button>').join("")||'<p class="sheet-description">原生权限目录不可用，请在电脑端配置。</p>':row.capabilities.plan?'<button class="sheet-list-button" data-plan><span><strong>进入 Plan</strong><small>先分析和规划，其他权限模式由原生 Claude 管理</small></span></button>':'<p class="sheet-description">当前 Agent 未提供远程权限切换。</p>');
  else if(sheetType==="permission-confirm"&&permissionChoice)html=heading("启用 Full Access？")+'<p class="sheet-description">允许 Codex 在当前账户可访问范围内读写文件和访问网络，并停止按需审批。此操作影响电脑端同一会话的后续执行。</p><div class="confirm-actions"><button class="subtle-button" data-close-sheet>取消</button><button class="danger-button" data-confirm-permission>确认完全访问</button></div>';
  else if(sheetType==="interaction"&&row){
    const item=row.interactions?.find(i=>i.id===sheetRequest);
    if(!item){closeSheet();return;}
    html=heading(esc(item.title))+'<div data-interaction-id="'+esc(item.id)+'">'+interactionHtml(item,writable()&&!pendingApprovals.has(current+":"+item.id),row)+'<p class="interaction-error" role="status">'+esc(interactionErrors.get(current+":"+item.id)??(pendingApprovals.has(current+":"+item.id)?"正在等待原生确认…":""))+'</p></div>';
  }
  else if(sheetType==="tasks"&&row)html=heading("任务记录")+(tasksHtml(row)||'<p class="sheet-description">暂无任务记录。</p>');
  else if(sheetType==="notices"&&row)html=heading("运行提示记录")+(row.notices??[]).slice().reverse().map(entry=>'<section class="notice-entry"><strong>'+esc(entry.title)+'</strong><p>'+esc(entry.message)+'</p></section>').join("");
  else if(sheetType==="task-detail"&&row){
    const task=row.tasks?.find(t=>t.id===sheetRequest);
    const output=taskOutputs.get(current+":"+row.sessionId+":"+sheetRequest);
    html=heading(esc(task?.title||"任务"))+'<p class="sheet-description">'+esc(taskLabel(task?.state??"unknown"))+(task?.model?' · '+esc(task.model):"")+(task&&taskTimingHtml(task)?' · '+taskTimingHtml(task):"")+'</p>'+(task?.message?'<pre class="detail-code">'+esc(task.message)+'</pre>':"");
    if(task?.kind==="shell"&&task.data?.outputFile)html+='<button class="subtle-button" data-task-output="'+esc(task.id)+'">'+(output?"刷新任务输出":"读取任务输出")+'</button>';
    if(output)html+='<p class="sheet-description">'+(output.truncated?"仅显示输出末尾 64 KiB":"任务输出")+'</p><pre class="detail-code task-output">'+esc(output.text||"暂时没有输出")+'</pre>';
    html+=(row.interactions??[]).filter(i=>i.agentId===sheetRequest).map(i=>'<button class="sheet-list-button" data-interaction-open="'+esc(i.id)+'"><span>'+esc(i.title)+' · 需要回答</span></button>').join("");
    html+='<div id="task-content"></div>';
  }
  else if(sheetType==="approval-detail"&&row){const a=row.approvals.find(a=>a.id===sheetRequest);if(!a){closeSheet();return;}html=heading("审批详情")+'<p class="sheet-description">'+esc(a.title)+'</p><pre class="detail-code">'+esc(a.detail)+'</pre><p class="sheet-description">批准仅针对这次操作。任一侧处理后，其余页面同步关闭待办。</p><div class="approval-actions" data-request="'+esc(a.id)+'">'+approvalButtons(a.id,!writable()||!row.capabilities.approvals)+'</div>';}
  else if(sheetType==="queue")html=heading(row?.capabilities.queue?"原生输入队列":"已追加的消息")+'<p class="sheet-description">'+(row?.capabilities.queue?"与电脑端共享的真实队列。执行前可以编辑、删除、调整顺序。":"Agent 已接受这些输入，后续安排由 Claude 决定。")+'</p>'+(row?.capabilities.queue?(row.queuedInputs??[]).map((item,index)=>'<section class="queued-item" data-queue-id="'+esc(item.id)+'"><textarea name="queue-'+esc(item.id)+'" aria-label="队列消息" maxlength="32768">'+esc(item.text)+'</textarea><div class="interaction-decisions"><button class="subtle-button" data-queue-operation="update">保存</button><button class="subtle-button" data-queue-operation="delete">删除</button><button class="subtle-button" data-queue-operation="up" '+(!index?"disabled":"")+'>上移</button><button class="subtle-button" data-queue-operation="start">立即执行</button></div></section>').join(""):(queues.get(current)??[]).map((text,i)=>'<div class="detail-code">'+(i+1)+'. '+esc(text)+'</div>').join(""))+'<p class="sheet-footnote">停止当前回合不会自动清空队列。</p>';
  else if(sheetType==="more")html=heading("会话选项")+'<button class="menu-item" data-open="sessions">'+icon("list")+'所有会话</button><button class="menu-item" data-open="info">'+icon("monitor")+'会话信息</button>'+(row?.tasks?.length?'<button class="menu-item" data-open="tasks">'+icon("list")+'任务记录<span class="menu-value">'+row.tasks.length+' 项</span></button>':"")+(row?.notices?.length?'<button class="menu-item" data-open="notices">'+icon("list")+'运行提示记录<span class="menu-value">'+row.notices.length+' 条</span></button>':"")+'<button class="menu-item" data-open="display">'+icon("settings")+'显示设置</button><button class="menu-item" data-copy-link>'+icon("copy")+'复制会话链接</button><button class="menu-item" data-reconnect>'+icon("refresh")+'重新连接</button>'+(row?.capabilities.end?'<button class="menu-item danger" data-open="end-confirm" '+(!writable()?"disabled":"")+'>'+icon("stop")+'结束 Agent 会话</button>':"");
  else if(sheetType==="end-confirm"&&row)html=heading("结束 Agent 会话？")+'<p class="sheet-description">将结束电脑端的「'+esc(row.title)+'」。网页保留已显示的内容。仅中断当前回复请使用「停止生成」。</p><div class="confirm-actions"><button class="subtle-button" data-close-sheet>取消</button><button class="danger-button" data-end>结束会话</button></div>';
  else if(sheetType==="info")html=heading("会话信息")+(row?'<pre class="detail-code">'+esc(row.title)+'\n'+esc(row.cwd)+'\n'+esc(row.agent)+' · '+esc(row.model)+'\n执行权限：'+esc(policyLabel(row))+'\n网页：'+(row.allowControl?"允许操作":"只读")+'</pre>':'<p class="sheet-description">暂无共享会话。</p>');
  else if(sheetType==="display")html=heading("显示设置")+'<p class="sheet-description">只改变网页消息，不影响电脑终端。</p><div class="menu-item">消息字号<span class="menu-value"><button class="subtle-button" data-font="-1" aria-label="减小字号">−</button> '+displaySize+' px <button class="subtle-button" data-font="1" aria-label="增大字号">+</button></span></div>';
  byId("sheet").innerHTML=html;
  if(sheetType==="task-detail"&&row){
    const task=row.tasks?.find(entry=>entry.id===sheetRequest),key=JSON.stringify([row.runtimeId,row.sessionId,sheetRequest]),placeholder=byId("task-content");
    if(taskTimeline?.key===key)placeholder.replaceWith(taskTimeline.node);
    else taskTimeline={key,node:placeholder,view:new TimelineView(placeholder)};
    taskTimeline.view.render(row,conversationItems(row).filter(entry=>task?.kind==="shell"?entry.id===task.data?.itemId:entry.agentId===sheetRequest));
  }
  restoreInteractionDraft();
  if(row)updateTaskTimers(row);

}
function updateSheet():void{
  if(!sheetType)return;
  const row=active(),key=sheetType==="interaction"?JSON.stringify([current,row?.sessionId,sheetRequest,row?.interactions?.find(i=>i.id===sheetRequest),writable(),[...pendingApprovals]]):JSON.stringify([sheetType,sheetRequest,roster.map(r=>[r.runtimeId,r.title,r.state,r.allowControl]),row?.approvals,row?.interactions,row?.tasks,row?.notices,row?.permissions,row?.queuedInputs,sheetType==="task-detail"?row?.items.filter(i=>i.agentId===sheetRequest||i.id===row?.tasks.find(task=>task.id===sheetRequest)?.data?.itemId):undefined,row?.allowControl,row?.model,row?.policy,[...pendingApprovals]]);
  if(sheetKey===key)return;sheetKey=key;
  const focused=document.activeElement as HTMLInputElement|null,fieldIndex=[...byId("sheet").querySelectorAll("input,textarea,select")].indexOf(focused!),selection=focused&&typeof focused.selectionStart==="number"?[focused.selectionStart,focused.selectionEnd]:null,keyAttr=focused?.dataset.select,decision=focused?.dataset.decision,focusedKey=focused?.dataset.fieldKey;
  const detailState=[...byId("sheet").querySelectorAll<HTMLDetailsElement>("details:not(.question-other)")].map(detail=>detail.open);
  const sheetScroll=byId("sheet").scrollTop,interactionScroll=byId("sheet").querySelector(".interaction-scroll")?.scrollTop??0;
  saveInteractionDraft();fillSheet();byId("sheet").scrollTop=sheetScroll;const scroll=byId("sheet").querySelector(".interaction-scroll");if(scroll)scroll.scrollTop=interactionScroll;
  [...byId("sheet").querySelectorAll<HTMLDetailsElement>("details:not(.question-other)")].forEach((detail,index)=>{if(detailState[index]!=null)detail.open=detailState[index];});
  if(keyAttr)byId("sheet").querySelector<HTMLButtonElement>('[data-select="'+CSS.escape(keyAttr)+'"]')?.focus();
  else if(fieldIndex>=0){const fields=[...byId("sheet").querySelectorAll<HTMLInputElement>("input,textarea,select")],field=focusedKey?fields.find(field=>field.dataset.fieldKey===focusedKey):fields[fieldIndex];if(field?.getClientRects().length)field.focus({preventScroll:true});if(field&&selection&&["text","search","url","tel","password"].includes(field.type)||field?.tagName==="TEXTAREA"){try{field?.setSelectionRange(selection?.[0]??0,selection?.[1]??0);}catch{/* 原生类型不提供选区。 */}}}
  else if(decision!==undefined)byId("sheet").querySelector<HTMLButtonElement>('[data-decision="'+decision+'"]')?.focus();
}
function openSheet(type:string):void{
  saveCardDraft();
  if(!sheetType)beforeSheet=document.activeElement as HTMLElement;
  saveInteractionDraft();sheetType=type;sheetKey="";byId("sheet-layer").dataset.sheet=type;byId("sheet-layer").hidden=false;fillSheet();byId("sheet").querySelector<HTMLButtonElement>("button")?.focus();
}
function closeSheet():void{saveInteractionDraft();byId("sheet-layer").hidden=true;byId("sheet").replaceChildren();sheetType="";sheetKey="";cardKey="";if(beforeSheet?.isConnected)beforeSheet.focus();const row=active();if(row)renderInteractions(row);}
async function poll():Promise<void>{
  if(polling||!authenticated)return;polling=true;
  try{
    const data=await request<AgentRoster>("/api/agents");roster=data.sessions;
    const alive=new Set(roster.map(r=>r.runtimeId));
    for(const id of snapshots.keys())if(!alive.has(id))snapshots.delete(id);
    connected=true;
    if(!wasConnected){showNotice();wasConnected=true;}
    if(!current||!alive.has(current)){if(roster.length||current)select(roster[0]?.runtimeId??"");}
    renderRoster(roster);updateSheet();
    const id=current;
    if(id){
      const seq=snapshots.get(id)?.seq;
      const result=await request<{unchanged?:boolean;snapshot?:AgentSession;changes?:AgentChange[];clockMs?:number}>("/api/agents/"+id+(seq===undefined?"":"?seq="+seq));
      if(typeof result.clockMs==="number")clockAnchors.set(id,{received:performance.now(),clock:result.clockMs});
      let next=result.snapshot;
      if(result.changes){const old=snapshots.get(id);next=old?applyChanges(old,result.changes)??undefined:undefined;if(!next)next=(await request<{snapshot:AgentSession}>("/api/agents/"+id)).snapshot;}
      if(next){if(next.protocolVersion!==1)throw new Error("Agent 协议版本不兼容，请更新 sTerm");if(typeof result.clockMs!=="number"&&result.snapshot&&typeof next.clockMs==="number")clockAnchors.set(id,{received:performance.now(),clock:next.clockMs});snapshots.set(id,next);if(id===current)render(next);}
    }
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
  const target=(event.target as Element).closest<HTMLButtonElement>("button");
  if(target?.disabled)return;
  if(target?.id==="composer-toggle"){
    if(!composerRequest)return;
    if(expandedComposers.has(composerRequest)){expandedComposers.delete(composerRequest);target.focus();}
    else expandedComposers.add(composerRequest);
    syncControls();if(!byId("agent-composer").hidden)draft.focus();return;
  }
  if(target?.dataset.taskOutput){const row=active(),taskId=target.dataset.taskOutput;if(!row||!connected)return;target.disabled=true;void request<{text:string;truncated?:boolean}>("/api/agents/"+encodeURIComponent(row.runtimeId)+"/tasks/"+encodeURIComponent(taskId)+"/output").then(output=>{taskOutputs.set(row.runtimeId+":"+row.sessionId+":"+taskId,output);if(taskOutputs.size>32)taskOutputs.delete(taskOutputs.keys().next().value!);if(current===row.runtimeId&&sheetType==="task-detail"&&sheetRequest===taskId)fillSheet();}).catch(error=>{showNotice(error instanceof Error?error.message:String(error));target.disabled=false;});return;}
  if(target?.dataset.interactionOpen){saveCardDraft();cardRequest=target.dataset.interactionOpen;const row=active();if(row){const key=current+":"+row.sessionId+":"+cardRequest;collapsedInteractions.delete(key);if(sheetType==="task-detail")closeSheet();cardKey="";renderInteractions(row);}return;}
  if(target?.hasAttribute("data-collapse-interaction")){saveCardDraft();collapsedInteractions.add(current+":"+active()?.sessionId+":"+cardRequest);cardKey="";const row=active();if(row)renderInteractions(row);return;}
  if(target?.hasAttribute("data-expand-interaction")){sheetRequest=cardRequest;openSheet("interaction");cardKey="";const row=active();if(row)renderInteractions(row);return;}
  if(target?.dataset.task){sheetRequest=target.dataset.task;openSheet("task-detail");return;}

  const button=(event.target as Element).closest<HTMLButtonElement>("button");if(!button||button.disabled)return;
  if(button.hasAttribute("data-block-continue")){
    const row=active();if(!row||!writable(row))return;
    const review=blockReview(row),card=byId("blocked-card");
    if(!review.canContinue||card.dataset.review!==review.key||!card.querySelector<HTMLInputElement>("[data-block-acknowledge]")?.checked||!card.querySelector<HTMLDetailsElement>(".block-findings")?.open||pendingBlocks.has(review.key))return;
    pendingBlocks.add(review.key);renderBlocked(row);syncControls();
    void command({action:"continueBlocked",turnId:review.turnId,reviewId:review.reviewId},row).catch(()=>{}).finally(()=>{pendingBlocks.delete(review.key);const latest=active();if(latest)renderBlocked(latest);syncControls();});return;
  }
  if(button.dataset.open){openSheet(button.dataset.open);return;}
  if(button.hasAttribute("data-close-sheet")){closeSheet();return;}
  if(button.dataset.select){select(button.dataset.select);void poll();return;}
  if(button.dataset.model){const value=button.dataset.model;closeSheet();void command({action:"setModel",model:value}).catch(()=>{});return;}
  if(button.dataset.permission!==undefined){
    permissionChoice=active()?.permissions?.options?.[Number(button.dataset.permission)];
    if(!permissionChoice?.allowed)return;
    if(permissionChoice.profile===":danger-full-access")openSheet("permission-confirm");else void applyPermission();
    return;
  }
  if(button.hasAttribute("data-confirm-permission")){void applyPermission();return;}
  const interactionId=button.closest<HTMLElement>("[data-interaction-id]")?.dataset.interactionId;
  const interaction=active()?.interactions?.find(item=>item.id===interactionId);
  if(interaction){
    if(button.dataset.questionNav){const root=button.closest<HTMLElement>("[data-interaction-id]")!;navigateQuestion(root,interaction,Number(button.dataset.questionNav));return;}
    if(button.dataset.interactionDecision!==undefined){void answerInteraction(interaction,{decision:interaction.data.decisions?.[Number(button.dataset.interactionDecision)]?.value});return;}
    if(button.dataset.permissionScope){void answerInteraction(interaction,{scope:button.dataset.permissionScope});return;}
    if(button.dataset.formAction){void answerInteraction(interaction,{action:button.dataset.formAction});return;}
    if(button.hasAttribute("data-interaction-cancel")){void answerInteraction(interaction,{cancel:true});return;}
  }
  if(button.dataset.queueOperation){
    const row=active(),element=button.closest<HTMLElement>("[data-queue-id]"),queueId=element?.dataset.queueId,operation=button.dataset.queueOperation;
    if(!row||!queueId||!writable())return;
    if(operation==="up"){const ids=(row.queuedInputs??[]).map(i=>i.id),index=ids.indexOf(queueId);if(index>0){[ids[index-1],ids[index]]=[ids[index],ids[index-1]];void command({action:"queue",operation:"reorder",ids}).catch(()=>{});}}
    else if(["delete","start","update"].includes(operation))void command({action:"queue",operation:operation as "update"|"delete"|"start",queueId,text:element?.querySelector("textarea")?.value}).catch(()=>{});
    return;
  }
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
function formDraftKey():string{return current+":"+(active()?.sessionId??"")+":"+sheetRequest+":"+sheetType;}
function captureFields(root:HTMLElement):InteractionDraft{
  return {fields:[...root.querySelectorAll<HTMLInputElement|HTMLTextAreaElement|HTMLSelectElement>("input,textarea,select")].map(field=>({key:field.dataset.fieldKey??field.name,value:field.value,checked:field instanceof HTMLInputElement&&field.checked,selected:field instanceof HTMLSelectElement?[...field.options].map(option=>option.selected):undefined})),questionnaire:questionnaireView(root)};
}
function saveInteractionDraft():void{
  saveCardDraft();
  if(!["interaction","queue"].includes(sheetType))return;
  interactionDrafts.set(formDraftKey(),captureFields(byId("sheet")));
}
function restoreInteractionDraft():void{restoreFields(byId("sheet"),formDraftKey());}
function interactionFeedback(item:AgentInteraction,message=""):void{
  const key=current+":"+item.id;if(message)interactionErrors.set(key,message);else interactionErrors.delete(key);
  for(const root of document.querySelectorAll<HTMLElement>("[data-interaction-id]"))if(root.dataset.interactionId===item.id){
    const status=root.querySelector<HTMLElement>(".interaction-error");if(status)status.textContent=message;
  }
  scheduleInteractionLayout();
}
function navigateQuestion(root:HTMLElement,item:AgentInteraction,direction:number):void{
  if(pendingApprovals.has(current+":"+item.id))return;
  const page=questionPage(root,item);
  if(direction>0&&writable()&&unansweredQuestion(root,item,page)>=0){interactionFeedback(item,"请先回答当前问题");return;}
  interactionFeedback(item);showQuestion(root,item,page+direction);saveInteractionDraft();
}
async function answerInteraction(item:AgentInteraction,response:unknown):Promise<void>{
  const row=active();if(!row||!writable())return;
  const key=current+":"+item.id;if(pendingApprovals.has(key))return;
  saveInteractionDraft();pendingApprovals.add(key);interactionErrors.delete(key);syncControls();cardKey="";renderInteractions(row);
  try{await command({action:"respondInteraction",requestId:item.id,response},row);}
  catch(error){interactionErrors.set(key,error instanceof Error?error.message:String(error));}
  finally{pendingApprovals.delete(key);syncControls();updateSheet();cardKey="";const latest=active();if(latest)renderInteractions(latest);}
}
async function applyPermission():Promise<void>{
  const row=active(),choice=permissionChoice;if(!row||!choice)return;
  closeSheet();await command({action:"setPermissions",profile:choice.profile,approvalPolicy:choice.approvalPolicy,reviewer:choice.reviewer},row).catch(()=>{});
}
function renderInteractions(row:AgentSession):void{
  const dock=byId("interaction-dock"),items=(row.interactions??[]).filter(item=>row.activity?.kind!=="blocked"||Boolean(item.agentId));
  const card=byId("interaction-card");dock.hidden=!items.length;card.hidden=!items.length;
  const key=JSON.stringify(items.map(i=>[i.id,i.title,i.agentId,i.blocking]));
  if(dock.dataset.key!==key){
    dock.dataset.key=key;dock.innerHTML=items.map(item=>'<button class="interaction-prompt" data-interaction-open="'+esc(item.id)+'"><span><strong>'+esc(item.title)+'</strong><small>'+(item.agentId?"子 Agent · ":"")+(item.blocking?"等待回答":"可随时回答")+'</small></span><span>查看</span></button>').join("");
  }
  if(sheetType==="interaction"&&!items.some(i=>i.id===sheetRequest)){const key=formDraftKey();closeSheet();interactionDrafts.delete(key);}
  for(const key of interactionDrafts.keys())if(key.startsWith(row.runtimeId+":")&&!items.some(i=>key===row.runtimeId+":"+row.sessionId+":"+i.id+":interaction")&&!key.endsWith(":queue"))interactionDrafts.delete(key);
  if(interactionDrafts.size>128)interactionDrafts.delete(interactionDrafts.keys().next().value!);
  if(!items.some(item=>item.id===cardRequest))cardRequest=items.find(item=>item.blocking)?.id??items[0]?.id??"";
  const selected=items.find(item=>item.id===cardRequest);
  if(!selected){card.replaceChildren();delete card.dataset.draftKey;delete card.dataset.interactionId;cardKey="";return;}
  const selectionKey=current+":"+row.sessionId+":"+cardRequest;
  const collapsed=collapsedInteractions.has(selectionKey);
  const inSheet=sheetType==="interaction"&&sheetRequest===cardRequest;
  const nativeItemId=(selected.data.context as Record<string,unknown>|undefined)?.itemId;
  const signature=JSON.stringify([current,selected,collapsed,inSheet,writable(),pendingApprovals.has(current+":"+cardRequest),interactionErrors.get(current+":"+cardRequest),row.items.find(item=>(item.agentId||"")===(selected.agentId||"")&&(item.id===nativeItemId||item.data?.id===nativeItemId))?.data]);
  if(cardKey===signature)return;
  const sameRequest=card.dataset.draftKey===selectionKey+":interaction";
  const focused=document.activeElement as HTMLInputElement|null,fields=[...card.querySelectorAll("input,textarea,select,button")],fieldIndex=sameRequest?fields.indexOf(focused!):-1,selection=focused&&typeof focused.selectionStart==="number"?[focused.selectionStart,focused.selectionEnd!]:undefined,focusedKey=focused?.dataset.fieldKey;
  const details=sameRequest?[...card.querySelectorAll<HTMLDetailsElement>("details:not(.question-other)")].map(detail=>detail.open):[],scroll=sameRequest?(card.querySelector(".interaction-scroll")??card.querySelector(".interaction-card-body"))?.scrollTop??0:0;
  saveCardDraft();cardKey=signature;
  card.dataset.kind=selected.kind;card.dataset.interactionId=cardRequest;card.dataset.draftKey=current+":"+row.sessionId+":"+cardRequest+":interaction";
  card.innerHTML='<div class="interaction-card-heading"><div><strong>'+esc(selected.title)+'</strong><small>'+esc(selected.agentId?"子 Agent":"主 Agent")+' · '+((selected.data.questions?.length??0)>1?(collapsed||inSheet?"共 "+selected.data.questions!.length+" 题":'<span data-question-progress role="status"></span>'):selected.blocking?"等待回答":"可随时回答")+(items.length>1?' · 待办 '+(items.findIndex(item=>item.id===cardRequest)+1)+'/'+items.length:"")+'</small></div><div>'+(collapsed||inSheet?'<button class="touch-button" data-interaction-open="'+esc(cardRequest)+'">展开</button>':'<button class="touch-button" data-expand-interaction aria-label="放大详情">'+icon("expand")+'</button><button class="touch-button" data-collapse-interaction aria-label="收起待办">'+icon("down")+'</button>')+'</div></div>'+(collapsed||inSheet?"":'<div class="interaction-card-body">'+interactionHtml(selected,writable()&&!pendingApprovals.has(current+":"+cardRequest),row)+'</div>')+'<p class="interaction-error" role="status">'+esc(interactionErrors.get(current+":"+cardRequest)??(pendingApprovals.has(current+":"+cardRequest)?"正在等待原生确认…":""))+'</p>';
  restoreFields(card,card.dataset.draftKey);
  if(fieldIndex>=0){const fields=[...card.querySelectorAll<HTMLInputElement>("input,textarea,select,button")],field=focusedKey?fields.find(field=>field.dataset.fieldKey===focusedKey):fields[fieldIndex];if(field?.getClientRects().length)field.focus({preventScroll:true});if(selection&&field?.setSelectionRange&&(["text","search","url","tel","password"].includes(field.type)||field.tagName==="TEXTAREA"))field.setSelectionRange(selection[0],selection[1]);}
  [...card.querySelectorAll<HTMLDetailsElement>("details:not(.question-other)")].forEach((detail,index)=>{if(details[index]!=null)detail.open=details[index];});
  const scrollBody=card.querySelector(".interaction-scroll")??card.querySelector(".interaction-card-body");if(scrollBody)scrollBody.scrollTop=scroll;
}

function saveCardDraft():void{
  const card=byId("interaction-card");if(!card?.dataset.draftKey||!card.querySelector("input,textarea,select"))return;
  interactionDrafts.set(card.dataset.draftKey,captureFields(card));
}
function restoreFields(root:HTMLElement,key:string):void{
  const saved=interactionDrafts.get(key),values=new Map(saved?.fields.map(field=>[field.key,field]));
  for(const field of root.querySelectorAll<HTMLInputElement|HTMLTextAreaElement|HTMLSelectElement>("input,textarea,select")){
    const value=values.get(field.dataset.fieldKey??field.name);if(!value)continue;
    if(field instanceof HTMLInputElement&&["radio","checkbox"].includes(field.type))field.checked=value.checked;else field.value=value.value;
    if(field instanceof HTMLSelectElement&&value.selected)[...field.options].forEach((option,position)=>option.selected=Boolean(value.selected?.[position]));
  }
  const item=active()?.interactions?.find(item=>item.id===root.querySelector<HTMLFormElement>("#interaction-form")?.dataset.interaction);
  if(item){syncQuestionnaire(root,item,saved?.questionnaire);for(const button of root.querySelectorAll<HTMLButtonElement>("[data-question-nav]"))button.disabled=pendingApprovals.has(current+":"+item.id);}
}
function updateActivity(row:AgentSession):void{
  const turn=[...(row.turns??[])].reverse().find(entry=>!entry.agentId),anchor=clockAnchors.get(row.runtimeId);
  const elapsed=turn?.startedAtMs!=null?(anchor?anchor.clock+performance.now()-anchor.received:row.clockMs??turn.startedAtMs)-turn.startedAtMs:undefined;
  const timing=busy(row)?durationLabel(elapsed):recordDuration(turn);
  const tasks=liveTasks(row).filter(task=>runningStates.includes(task.state));
  const state=row.state==="idle"?(turn?"本轮回复已完成":"会话空闲"):labels[row.state]??row.state;
  byId("activity-text").textContent=[state,timing?(turn?.timeSource==="observed"||turn?.timeSource==="nativeDuration"&&busy(row)?"约 ":"")+timing:"",tasks.length?tasks.length+" 项任务仍在运行":""].filter(Boolean).join(" · ");
  byId("activity-bar").hidden=!busy(row);
  let summary=byId("turn-summary");
  if(!summary){summary=document.createElement("p");summary.id="turn-summary";summary.className="turn-summary";messages.append(summary);}
  summary.hidden=busy(row)||!turn||runningStates.includes(turn.state);
  summary.textContent=[state,timing?(turn?.timeSource==="observed"?"约 ":"")+timing:"",byId("usage-summary").textContent??""].filter(Boolean).join(" · ");
  if(!summary.hidden&&messages.lastElementChild!==summary)messages.append(summary);
  updateTaskTimers(row);
}
function updateTaskTimers(row:AgentSession):void{
  const anchor=clockAnchors.get(row.runtimeId),now=anchor?anchor.clock+performance.now()-anchor.received:row.clockMs;
  if(typeof now!=="number")return;
  for(const element of document.querySelectorAll<HTMLElement>("[data-task-started]"))element.textContent=(element.dataset.timeSource==="observed"?"约 ":"")+durationLabel(Math.max(0,now-Number(element.dataset.taskStarted)));
}
setInterval(()=>{const row=active();if(row)updateActivity(row);},1000);
function interactionInput(target:EventTarget|null):void{
  if(!(target instanceof Element))return;
  const root=target.closest<HTMLElement>("[data-interaction-id]"),item=active()?.interactions?.find(item=>item.id===root?.dataset.interactionId);
  if(root&&item){
    syncQuestionnaire(root,item);
    if(["请先回答当前问题","请回答全部问题"].includes(interactionErrors.get(current+":"+item.id)??"")&&unansweredQuestion(root,item,questionPage(root,item))<0)interactionFeedback(item);
  }
  if(byId("sheet").contains(target))saveInteractionDraft();else if(byId("interaction-card").contains(target))saveCardDraft();
}
document.addEventListener("input",event=>interactionInput(event.target));
document.addEventListener("change",event=>{if(byId("blocked-card").contains(event.target as Node))syncControls();else interactionInput(event.target);});
document.addEventListener("toggle",event=>{if((event.target as Element)?.matches(".question-other"))interactionInput(event.target);},true);
document.addEventListener("submit",event=>{
  const form=event.target as HTMLFormElement;if(form.id!=="interaction-form")return;
  event.preventDefault();const item=active()?.interactions?.find(i=>i.id===form.dataset.interaction);if(!item||!writable()||pendingApprovals.has(current+":"+item.id))return;
  const root=form.closest<HTMLElement>("[data-interaction-id]")!;
  if(item.data.questions?.length&&questionPage(root,item)<item.data.questions.length-1){navigateQuestion(root,item,1);return;}
  try{void answerInteraction(item,readInteraction(item,form));}catch(error){
    const missing=unansweredQuestion(root,item);if(missing>=0)showQuestion(root,item,missing);
    interactionFeedback(item,error instanceof Error?error.message:String(error));saveInteractionDraft();
  }
});
byId("queue-add").onclick=()=>{
  const row=active(),text=draft.value,id=current;if(!row?.capabilities.queue||!text.trim()||submitting.has(id))return;
  submitting.add(id);syncControls();
  void command({action:"queue",operation:"add",text},row).then(()=>{if(current===id&&draft.value===text){draft.value="";saveDraft(id,"");}}).catch(()=>{}).finally(()=>{submitting.delete(id);syncControls();});
};

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
byId("dismiss-native-notice").onclick=()=>{
  const key=byId("native-notice").dataset.noticeKey;if(key)dismissedNotices.add(key);
  if(dismissedNotices.size>256)dismissedNotices.delete(dismissedNotices.values().next().value!);
  transientNotice=undefined;const row=active();if(row)renderNotices(row);
};
byId("sheet-layer").querySelector(".sheet-scrim")!.addEventListener("click",()=>closeSheet());
document.addEventListener("keydown",event=>{
  if(event.key==="Escape"&&sheetType){event.preventDefault();closeSheet();}
  if(event.key==="Tab"&&sheetType){const nodes=[...byId("sheet").querySelectorAll<HTMLButtonElement>("button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],summary")].filter(node=>node.getClientRects().length>0),first=nodes[0],last=nodes[nodes.length-1];if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}
});
let interactionLayoutFrame=0;
function scheduleInteractionLayout():void{
  if(interactionLayoutFrame)return;
  interactionLayoutFrame=requestAnimationFrame(()=>{
    interactionLayoutFrame=0;
    const layout=byId("agent-layout"),cards=[byId("interaction-card"),byId("blocked-card")].filter(card=>!card.hidden);
    if(layout.hidden||!cards.length)return;
    const height=layout.getBoundingClientRect().height;
    const outerHeight=(node:HTMLElement):number=>{if(!node.getClientRects().length)return 0;const style=getComputedStyle(node);return node.getBoundingClientRect().height+(parseFloat(style.marginTop)||0)+(parseFloat(style.marginBottom)||0);};
    const fixed=[...layout.children].filter((node):node is HTMLElement=>node instanceof HTMLElement&&node.id!=="conversation"&&!cards.includes(node)).reduce((sum,node)=>sum+outerHeight(node),0);
    const margins=cards.reduce((sum,node)=>{const style=getComputedStyle(node);return sum+(parseFloat(style.marginTop)||0)+(parseFloat(style.marginBottom)||0);},0);
    const history=Math.min(96,Math.max(30,height*.14)),budget=Math.max(0,(height-fixed-margins-history)/cards.length);
    for(const card of cards)card.style.setProperty("--interaction-max-height",Math.floor(budget)+"px");
  });
}
function viewport():void{
  const height=window.visualViewport?.height??window.innerHeight,app=byId("agent-app");
  document.documentElement.style.setProperty("--viewport-height",height+"px");
  app.dataset.compactViewport=String(height<=520);
  const editing=document.activeElement?.closest("#interaction-form");
  app.dataset.interactionKeyboard=String(Boolean(editing&&innerWidth<=520&&innerHeight-height>120));
  scheduleInteractionLayout();
}
const interactionResize=new ResizeObserver(scheduleInteractionLayout);
interactionResize.observe(byId("agent-layout"));
for(const node of byId("agent-layout").children)if(node.id!=="conversation"&&node.id!=="interaction-card"&&node.id!=="blocked-card")interactionResize.observe(node);
document.addEventListener("focusin",viewport);document.addEventListener("focusout",()=>requestAnimationFrame(viewport));
window.visualViewport?.addEventListener("resize",viewport);window.addEventListener("resize",viewport);viewport();
const querySession=new URLSearchParams(location.search).get("session");if(querySession)current=querySession;
draft.value=restoreDraft(current);
window.setInterval(()=>void poll(),350);
const pair=new URLSearchParams(location.hash.slice(1)).get("pair");
if(pair)void pairRemoteBrowser(pair,false).then(()=>{history.replaceState(null,"",location.pathname+location.search);void authenticate();}).catch(error=>void login.check(String(error)));
else void authenticate();
