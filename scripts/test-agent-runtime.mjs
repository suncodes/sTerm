import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({ entryPoints: ["src/agent/state.ts"], bundle: true, write: false, format: "esm", platform: "node" });
const { applyChanges } = await import("data:text/javascript;base64," + Buffer.from(bundle.outputFiles[0].contents).toString("base64"));
const session = () => ({ runtimeId: "r", sessionId: "s", generation: 1, seq: 4, settingsRevision: 2, model: "a", policy: "", state: "running", turnId: "turn", items: [{ id: "a", kind: "assistant", text: "one" }], approvals: [], capabilities: {} });

test("ordered live blocks, native timestamps and unknown background state converge after replay",()=>{
  const events=[{seq:5,events:[
    {type:"upsert",item:{id:"thinking",kind:"thinking",text:"summary",data:{startedAtMs:100}}},
    {type:"upsert",item:{id:"tool",kind:"tool",text:"output",data:{command:"echo one",startedAtMs:200}}},
    {type:"turn",value:{id:"turn",agentId:"",state:"inProgress",startedAtMs:100}},
    {type:"task",task:{id:"shell",kind:"shell",state:"running",data:{startedAtMs:200}}}
  ]},{seq:6,events:[
    {type:"upsert",item:{id:"thinking",kind:"thinking",text:"finished summary",data:{startedAtMs:null,completedAtMs:300}}},
    {type:"turn",value:{id:"turn",agentId:"",state:"completed",startedAtMs:null,completedAtMs:400,durationMs:300}},
    {type:"state",state:"idle",turnId:""}
  ]}];
  const original=session(),next=applyChanges(original,events);
  assert.deepEqual(next.items.map(item=>item.id),["a","thinking","tool"]);
  assert.equal(next.items[1].data.startedAtMs,100);assert.equal(next.turns[0].durationMs,300);assert.equal(next.turns[0].startedAtMs,100);
  assert.equal(next.tasks[0].state,"running","parent completion completed a shell");
  const offline=applyChanges(next,[{seq:7,events:[{type:"state",state:"offline",turnId:""}]}]);
  assert.equal(offline.tasks[0].state,"unknown");assert.equal(original.items.length,1);
});

test("incremental reconnect deduplicates replay, rejects gaps, and preserves previous snapshots", () => {
  const original = session();
  const event = { seq: 5, events: [{ type: "delta", id: "a", delta: " two" }] };
  const next = applyChanges(original, [event, event]);
  assert.equal(next.items[0].text, "one two");
  assert.equal(original.items[0].text, "one");
  assert.equal(applyChanges(original, [{ ...event, seq: 6 }]), null);
});

test("completed command history cannot prevent new running child tasks from appearing",()=>{
  const original={...session(),tasks:Array.from({length:64},(_,index)=>({id:"old-"+index,kind:"shell",state:index===0?"running":"completed"}))};
  const result=applyChanges(original,[{seq:5,events:[{type:"task",task:{id:"child",kind:"agent",state:"running"}}]}]);
  assert.equal(result.tasks.length,64);assert(result.tasks.some(task=>task.id==="old-0"));assert(result.tasks.some(task=>task.id==="child"));
});

test("native approval closure and settings changes converge after incremental replay", () => {
  const next = applyChanges(session(), [
    { seq: 5, events: [{ type: "approval", approval: { id: "p" } }] },
    { seq: 6, events: [{ type: "resolved", id: "p" }, { type: "settings", model: "b", policy: "readOnly" }] },
    { seq: 7, events: [{ type: "state", state: "idle", turnId: "" }] },
  ]);
  assert.deepEqual(next.approvals, []);
  assert.equal(next.state, "idle");
  assert.equal(next.model, "b");
  assert.equal(next.settingsRevision, 3);
});
test("typed questions, retries, subagent content and queue converge without claiming completion", () => {
  const original = session();
  const result = applyChanges(original, [
    { seq: 5, events: [{ type: "interaction", interaction: { id: "q", kind: "question", agentId: "child", blocking: true } }, { type:"task", task:{ id:"child", state:"waiting" } }] },
    { seq: 6, events: [{ type: "upsert", item: { id:"child:a", agentId:"child", kind:"assistant", text:"child text" } },{ type:"queue",items:[{id:"one",text:"queued"}] }] },
    { seq: 7, events: [{ type:"activity",value:{kind:"retrying",message:"upstream retry"} },{type:"state",state:"retrying",turnId:"turn"}] },
    { seq: 8, events: [{ type:"items",items:[{id:"main",text:"main refreshed"}] },{type:"resolved",id:"q"}] }
  ]);
  assert.equal(result.state, "retrying");
  assert.equal(result.tasks[0].state, "waiting");
  assert.deepEqual(result.interactions, []);
  assert.equal(result.items.find(i=>i.agentId==="child").text,"child text");
  assert.equal(result.queuedInputs[0].id,"one");
  assert.equal(result.activity.message,"upstream retry");
  assert.equal(original.items.length,1);
});

test("session replacement clears old answers, child content and queue, and blocked state survives", () => {
  const original = {...session(), interactions:[{id:"old"}],tasks:[{id:"old-child"}],queuedInputs:[{id:"old-queue"}]};
  const result = applyChanges(original,[
    {seq:5,events:[{type:"ready",sessionId:"replacement",model:"a",models:[],capabilities:{}}]},
    {seq:6,events:[{type:"state",state:"blocked",turnId:""}]}
  ]);
  assert.equal(result.state,"blocked");
  assert.deepEqual([result.interactions,result.tasks,result.queuedInputs,result.items],[[],[],[],[]]);
});

const blockBundle = await build({ entryPoints: ["src/agent/blocking.ts"], bundle: true, write: false, format: "esm", platform: "node" });
const { blockReview, blockedHtml } = await import("data:text/javascript;base64," + Buffer.from(blockBundle.outputFiles[0].contents).toString("base64"));
const blockedSession = () => ({...session(),agent:"codex",state:"blocked",capabilities:{send:true},activity:{kind:"blocked",message:"This request was blocked by our safety systems. Reason: Potentially unintended activity.",turnId:"turn",reviewId:"review",details:{detailedExplanation:"Review <script>alert(1)</script> before continuing.",steer:{message:"Continue only within the requested scope."},reviewTarget:"opaque"}}});

test("safety review requires current native explanation and steer, never an opaque target or historical error alone",()=>{
  const original=blockedSession();assert.equal(blockReview(original).canContinue,true);
  const variants=[
    {...original,activity:{kind:"blocked",message:original.activity.message}},
    {...original,activity:{...original.activity,reviewId:null}},
    {...original,activity:{...original.activity,details:{reviewTarget:"opaque"}}},
    {...original,activity:{...original.activity,details:{detailedExplanation:"Explanation only"}}},
    {...original,activity:{...original.activity,continuationStatus:"submitted"}},
    {...original,turns:[{id:"new-turn",agentId:""}]},
    {...original,state:"running"},
    {...original,activity:{...original.activity,details:{...original.activity.details,steer:{message:"中".repeat(342)}}}},
  ];
  for(const variant of variants)assert.equal(blockReview(variant).canContinue,false);
  assert.notEqual(blockReview(original).key,blockReview({...original,activity:{...original.activity,reviewId:"updated"}}).key);
  assert.equal(blockReview({...original,allowControl:false}).canContinue,true,"read-only controls are disabled separately from native capability");
});

test("blocked card escapes native material, starts without acknowledgement and gives an actual session switch",()=>{
  const original=blockedSession(),html=blockedHtml(original,true,false);
  assert(!html.includes("<script>"));assert(html.includes("&lt;script&gt;"));
  assert(html.includes("data-block-acknowledge"));assert(!html.includes(" checked"));
  assert(html.includes("data-block-continue disabled"));assert(html.includes('data-open="sessions"'));
  const historical=blockedHtml({...original,activity:{kind:"blocked",message:original.activity.message}},true,false);
  assert(!historical.includes("data-block-continue"));assert(!historical.includes("等待电脑端处理"));
  assert(historical.includes(original.activity.message));
});

test("safety block and live review survive browser replay and clear only with the accepted next turn",()=>{
  const original=blockedSession();original.seq=4;
  const snapshot=JSON.parse(JSON.stringify(original));assert.equal(blockReview(snapshot).key,blockReview(original).key);
  const pending=applyChanges(original,[{seq:5,events:[{type:"activity",value:{...original.activity,continuationStatus:"submitted"}}]}]);
  assert.equal(blockReview(pending).canContinue,false);assert.equal(pending.state,"blocked");
  const running=applyChanges(pending,[{seq:6,events:[{type:"activity",value:{}},{type:"state",state:"running",turnId:"new-turn"}]}]);
  assert.equal(running.state,"running");assert.deepEqual(running.activity,{});
});

test("native turn ending and browser-only reattachment cannot enable a blocked session",()=>{
  const original=blockedSession();original.turns=[{id:"turn",agentId:"",state:"inProgress"}];
  assert.equal(blockReview(original).canContinue,false);
  assert(blockedHtml(original,true,false).includes("等待原生结束当前回合"));
  const attached=applyChanges({...original,seq:4},[{seq:5,events:[{type:"ready",sessionId:original.sessionId,model:"a",models:[],capabilities:{send:true}}]}]);
  assert.equal(attached.state,"blocked");assert.equal(attached.activity.reviewId,original.activity.reviewId);
});

const displayBundle = await build({entryPoints:["src/agent/display.ts"],bundle:true,write:false,format:"esm",platform:"node"});
const {parseQuestionReplies,conversationItems,liveTasks}=await import("data:text/javascript;base64,"+Buffer.from(displayBundle.outputFiles[0].contents).toString("base64"));
const noticeBundle = await build({entryPoints:["src/agent/notices.ts"],bundle:true,write:false,format:"esm",platform:"node"});
const {NoticeTracker,activeNotices}=await import("data:text/javascript;base64,"+Buffer.from(noticeBundle.outputFiles[0].contents).toString("base64"));
const reply=(call,index,question,answer)=>({questionItemId:JSON.stringify(["request_user_input_async",call,index]),question,answer});
const envelope=value=>"<send_user_message_question_reply>\n"+JSON.stringify(value)+"\n</send_user_message_question_reply>";

test("async answers require a complete valid envelope and native question identity",()=>{
  const one=reply("call",0,"Target?","D:");
  assert.equal(parseQuestionReplies(envelope(one))[0].answer,"D:");
  assert.equal(parseQuestionReplies("  "+envelope([one])+"  ")[0].call,"call");
  for(const text of [
    "Explain "+envelope([one]),envelope([]),envelope([one,{answer:"bad"}]),
    envelope({...one,questionItemId:'["another_tool","call",0]'}),
    envelope({...one,questionItemId:'["request_user_input_async","call",-1]'}),
    envelope({...one,questionItemId:'["request_user_input_async","call",0.5]'}),
    envelope({...one,answer:42}),"<send_user_message_question_reply>{broken}</send_user_message_question_reply>",
  ]) assert.equal(parseQuestionReplies(text),undefined,text);
});

test("live and refreshed async answers fold into the right agent question without mutating transport",()=>{
  const source={id:"call",kind:"assistant",text:"Choose target",agentId:"",data:{id:"call",questions:[{title:"Full target question"}]}};
  const child={...source,id:"child:call",agentId:"child",data:{...source.data}};
  const answer={id:"answer",kind:"user",text:envelope([reply("call",0,"Quoted target?","D:")]),agentId:""};
  const original={...session(),agent:"codex",items:[source,child]};
  const next=applyChanges(original,[{seq:5,events:[{type:"upsert",item:answer}]}]);
  const displayed=conversationItems(next);
  assert.deepEqual(displayed.map(item=>item.id),["call","child:call"]);
  assert.equal(displayed[0].data.questionReplies[0].question,"Full target question");
  assert.equal(displayed[0].data.questionReplies[0].answer,"D:");
  assert.equal(displayed[1].data.questionReplies,undefined);
  assert.equal(source.data.questionReplies,undefined);
  assert.equal(next.items[2].text,answer.text);
  assert.deepEqual(conversationItems(JSON.parse(JSON.stringify(next))),displayed);
});

test("unmatched async answers remain readable and mixed replies retain fallback and attachments",()=>{
  const source={id:"call",kind:"assistant",agentId:"",data:{questions:[{title:"Secret",secret:true}]}};
  const answer={id:"a",kind:"user",agentId:"",text:envelope([reply("call",0,"Secret?","never-display"),reply("missing",0,"Missing question","B")]),data:{content:[{type:"image",path:"attachment"}]}};
  const displayed=conversationItems({...session(),agent:"codex",items:[source,answer]});
  assert.equal(displayed.length,2);assert.equal(displayed[0].data.questionReplies[0].answer,"已提交秘密回答");
  assert(!JSON.stringify(displayed).includes("never-display"));
  assert.equal(displayed[1].text,"");assert.equal(displayed[1].data.questionReplies[0].question,"Missing question");
  assert.equal(displayed[1].data.questionReplies[0].answer,"B");assert.equal(displayed[1].data.content[0].type,"image");
  const literal={...answer,text:"Explain "+envelope([reply("call",0,"Q","A")])};
  assert.equal(conversationItems({...session(),agent:"codex",items:[literal]})[0].text,literal.text);
  assert.equal(conversationItems({...session(),agent:"claude",items:[answer]})[0].text,answer.text);
});

test("task activity excludes foreground commands and terminal history while preserving independent and uncertain work",()=>{
  const tasks=[
    {id:"foreground",kind:"shell",state:"running",data:{backgroundConfirmed:false}},
    {id:"complete",kind:"agent",state:"completed"},{id:"failed",kind:"agent",state:"failed"},
    {id:"background",kind:"shell",state:"running",data:{backgroundConfirmed:true}},
    {id:"child",kind:"agent",state:"waiting"},{id:"disconnected",kind:"agent",state:"unknown"},
  ];
  const row={...session(),state:"idle",tasks};
  assert.deepEqual(liveTasks(row).map(task=>task.id),["background","child","disconnected"]);
  assert.deepEqual(liveTasks({...row,tasks:tasks.map(task=>({...task,state:"completed"}))}),[]);
  assert.equal(row.tasks.length,6);
});

test("notices seed snapshots silently, ignore timestamp replay and report meaningful progress changes once",()=>{
  const tracker=new NoticeTracker(),notice={id:"review",title:"Review",phase:"running",message:"Reviewing",updatedAtMs:1};
  const row={...session(),notices:[notice]};
  assert.deepEqual(tracker.update(row),[]);
  assert.deepEqual(tracker.update({...row,notices:[{...notice,updatedAtMs:2}]}),[]);
  const completed={...notice,phase:"completed",message:"Done",updatedAtMs:3};
  assert.deepEqual(tracker.update({...row,notices:[completed]}),[completed]);
  assert.deepEqual(tracker.update({...row,notices:[completed]}),[]);
  assert.deepEqual(tracker.update({...row,sessionId:"another"}),[]);
  assert.deepEqual(tracker.update({...row,notices:[completed]}),[]);
});

test("progress strips follow current turns and independent children and clear after completion",()=>{
  const row={...session(),tasks:[{id:"child",state:"running"}],notices:[
    {id:"hook",phase:"running",turnId:"turn"},{id:"old",phase:"running",turnId:"old"},
    {id:"child-hook",phase:"running",agentId:"child"},{id:"done",phase:"completed"},
    {id:"info",message:"Historical notice"},
  ]};
  assert.deepEqual(activeNotices(row).map(notice=>notice.id),["hook","child-hook"]);
  assert.deepEqual(activeNotices({...row,state:"idle"}).map(notice=>notice.id),["child-hook"]);
  assert.deepEqual(activeNotices({...row,state:"idle",tasks:[{id:"child",state:"completed"}]}),[]);
});

const timelineBundle=await build({entryPoints:["src/agent/timeline.ts"],bundle:true,write:false,format:"esm",platform:"node"});
const {timelineItems,timelineEntries,identityItems}=await import("data:text/javascript;base64,"+Buffer.from(timelineBundle.outputFiles[0].contents).toString("base64"));
const activityBundle=await build({entryPoints:["src/agent/activity.ts"],bundle:true,write:false,format:"esm",platform:"node"});
const {commandText,commandDescription,commandOutput,processOutcome}=await import("data:text/javascript;base64,"+Buffer.from(activityBundle.outputFiles[0].contents).toString("base64"));
const shell=(id,status="completed",extra={})=>({id,kind:"tool",text:"echo one\n\nresult",status,turnId:"turn",agentId:"",data:{type:"commandExecution",command:"echo one",exitCode:0},...extra});

test("empty thinking is a live non-content placeholder and disappears after the matching turn ends",()=>{
  const empty={id:"thinking",kind:"thinking",text:"  ",status:"running",turnId:"turn",agentId:""};
  const row={...session(),state:"running",turnId:"turn",tasks:[{id:"child",state:"running",turnId:"child-turn"}]};
  assert.equal(timelineItems([empty],row).length,1);
  assert.equal(timelineItems([empty],{...row,state:"idle"}).length,0);
  assert.equal(timelineItems([{...empty,status:"completed"}],row).length,0);
  assert.equal(timelineItems([{...empty,turnId:"old"}],row).length,0);
  assert.equal(timelineItems([{...empty,agentId:"child",turnId:"child-turn"}],{...row,state:"idle"}).length,1);
  assert.equal(timelineItems([{...empty,agentId:"another"}],row).length,0);
  assert.equal(timelineItems([{...empty,text:"Public summary",status:"completed"}],{...row,state:"idle"}).length,1);
});

test("only adjacent successful commands from the same agent and turn share a group",()=>{
  const prose={id:"prose",kind:"assistant",turnId:"turn",text:"Explanation"};
  const entries=timelineEntries([shell("a"),shell("b"),prose,shell("c"),shell("fail","failed",{data:{command:"false",exitCode:1}}),shell("d"),shell("active","running"),shell("child","completed",{agentId:"child"}),shell("another","completed",{turnId:"another"}),shell("unknown","completed",{turnId:""})]);
  assert.deepEqual(entries.map(entry=>entry.items.map(item=>item.id)),[["a","b"],["prose"],["c"],["fail"],["d"],["active"],["child"],["another"],["unknown"]]);
  assert.equal(entries[0].key,"a");
  assert.deepEqual(timelineEntries([shell("a"),shell("b"),shell("c")])[0].items.map(item=>item.id),["a","b","c"]);
});

test("assistant identity appears once per reply across thinking and tools and returns on a new source or user message",()=>{
  const prose=(id,turnId="turn",agentId="")=>({id,kind:"assistant",turnId,agentId});
  const items=[prose("first"),{id:"thought",kind:"thinking"},shell("command"),prose("continued"),prose("new-turn","next"),prose("child","next","child"),prose("child-continuation","next","child"),{id:"user",kind:"user"},prose("after-user","next","child")];
  assert.deepEqual([...identityItems(items)],["first","new-turn","child","after-user"]);
});

test("shell previews use native descriptions and retain full commands and distinct output",()=>{
  const item=shell("shell","running",{data:{type:"claudeTool",input:{command:"echo one",description:"Check workspace"},exitCode:null}});
  assert.equal(commandText(item),"echo one");assert.equal(commandDescription(item),"Check workspace");
  assert.equal(commandOutput(item),"result");assert.equal(processOutcome(item),"running");
  assert.equal(commandOutput({...item,text:"echo one"}),"");
  assert.equal(commandOutput({...item,text:"Only output"}),"Only output");
  assert.equal(commandText({...item,data:{command:["git","status"]}}),"git status");
  assert.equal(processOutcome({...item,status:"completed",data:{exitCode:1}}),"failed");
});

test("native empty reasoning completion and snapshots preserve public streamed text without crossing identity",()=>{
  const thought={id:"thought",kind:"thinking",text:"Public summary",status:"running",turnId:"turn",agentId:"",data:{type:"reasoning",summary:["Public summary"],content:[]}};
  const original={...session(),items:[thought]},completed={...thought,text:"",status:"completed",data:{type:"reasoning",summary:[],content:[]}};
  const next=applyChanges(original,[{seq:5,events:[{type:"upsert",item:completed}]}]);
  assert.equal(next.items[0].text,"Public summary");assert.equal(next.items[0].status,"completed");
  const refreshed=applyChanges(next,[{seq:6,events:[{type:"items",items:[completed]}]}]);
  assert.equal(refreshed.items[0].text,"Public summary");
  assert.equal(original.items[0].status,"running");
  for(const entry of [{...completed,turnId:"another"},{...completed,agentId:"child"},{...completed,data:{type:"thinking"}}]){
    const result=applyChanges(original,[{seq:5,events:[{type:"upsert",item:entry}]}]);
    assert.equal(result.items[0].text,"");
  }
});
