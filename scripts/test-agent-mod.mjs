import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 同一公开 Mod 脚本通过模拟 $ API 驱动，验证请求协议，不复制实现。
async function modHarness(messages=[]) {
  const source = await readFile("integrations/claude-mod/hooks/register.js","utf8");
  const module = await import("data:text/javascript;base64,"+Buffer.from("const CONFIG={url:'http://bridge',};\n"+source+"\n//"+Math.random()).toString("base64"));
  const hooks=new Map(),events=[],timers=[],waiting=[],commands=[];
  const $={
    session:{id:async()=>"session",model:async()=>"mock",messages:async()=>messages},
    config:{list:async()=>[{key:"model",options:["mock"]}]},
    agent:{list:async()=>[]},env:{get:async()=>"isolated"},
    clock:{every:(_,cb)=>{timers.push(cb);return{cancel(){}}}},
    http:{fetch:async(url,init={})=>{
      if(url.endsWith("/event")){events.push(...JSON.parse(init.body).events);return{status:200,text:"{}"}}
      if(url.includes("/wait/")) return new Promise(resolve=>waiting.push(resolve));
      if(url.endsWith("/poll"))return{status:200,text:JSON.stringify({commands:commands.splice(0)})};
      return{status:200,text:"{}"};
    }}
  };
  module.register((name,fn)=>hooks.set(name,fn));
  const invoke=(name,e={},fn=async()=>({}))=>{
    const abort=new AbortController();Object.assign(fn,{signal:abort.signal,origin:{plugin:"engine"}});
    return {result:hooks.get(name)($,e,fn),abort};
  };
  await invoke("session.start").result;
  await invoke("turn.start",{turnId:"turn"}).result;
  const flush=async()=>{for(const timer of timers)timer();await new Promise(resolve=>setImmediate(resolve));};
  return{$,events,waiting,hooks,invoke,flush};
}

test("Claude AskUserQuestion uses the full question as answer key and supports multiple selections",async()=>{
  const h=await modHarness();
  const question={tool_name:"AskUserQuestion",tool_input:{questions:[{question:"Which target?",header:"Target",options:[{label:"A"},{label:"B"}]},{question:"Which features?",header:"Features",multiSelect:true,options:[{label:"X"},{label:"Y"}]}]}};
  const pending=h.invoke("classic.PermissionRequest",question);
  await h.flush();
  const card=h.events.find(e=>e.type==="interaction").interaction;
  assert.equal(card.kind,"question");assert.equal(card.data.questions[1].multiSelect,true);
  assert.equal(h.waiting.length,1);
  h.waiting.shift()({status:200,text:JSON.stringify({command:{id:"reply",response:{answers:{"0":["B"],"1":["X","Y"]}}}})});
  const result=await pending.result;
  assert.deepEqual(result,{decision:{behavior:"allow",updatedInput:{...question.tool_input,answers:{"Which target?":"B","Which features?":"X, Y"}}}});
  await h.invoke("turn.complete",{turnId:"turn",reason:"answer"}).result;await h.flush();
  assert(h.events.some(e=>e.type==="resolved"&&e.id===card.id));
});

test("Claude native cancellation removes pending requests and blocks a late web answer",async()=>{
  const h=await modHarness();let fallback=0;
  const pending=h.invoke("classic.PermissionRequest",{tool_name:"Write",tool_input:{file_path:"isolated"}},async()=>{fallback++;return{}});
  await h.flush();const card=h.events.find(e=>e.type==="interaction").interaction;
  pending.abort.abort();await h.flush();
  assert(h.events.some(e=>e.type==="resolved"&&e.id===card.id));
  h.waiting.shift()({status:200,text:JSON.stringify({command:{id:"late",response:{decision:"allow"}}})});
  await pending.result;assert.equal(fallback,1);
});

test("Claude hook remains pending beyond ten seconds by awaiting bridge HTTP", {timeout:16000},async()=>{
  const h=await modHarness();const pending=h.invoke("classic.PermissionRequest",{tool_name:"AskUserQuestion",tool_input:{questions:[{question:"Continue?",options:[{label:"Yes"}]}]}});
  await h.flush();await new Promise(resolve=>setTimeout(resolve,10500));
  assert.equal(h.waiting.length,1);
  h.waiting.shift()({status:200,text:JSON.stringify({command:{id:"later",response:{answers:{"0":["Yes"]}}}})});
  const answer=await pending.result;assert.equal(answer.decision.updatedInput.answers["Continue?"],"Yes");
});

test("Claude child elicitation and failure keep their source without changing the main activity",async()=>{
  const h=await modHarness();
  await h.invoke("turn.start",{agentId:"child",turnId:"child-turn"}).result;
  const pending=h.invoke("classic.Elicitation",{agent_id:"child",elicitation_id:"native-form",mode:"form",message:"Choose",requested_schema:{type:"object",properties:{name:{type:"string"}}}});
  await h.flush();
  const card=h.events.find(e=>e.type==="interaction").interaction;
  assert.equal(card.agentId,"child");assert.equal(card.turnId,"child-turn");assert.equal(card.kind,"form");
  h.waiting.shift()({status:200,text:JSON.stringify({command:{id:"answer",response:{action:"accept",content:{name:"A"}}}})});
  assert.deepEqual(await pending.result,{action:"accept",content:{name:"A"}});
  await h.invoke("classic.StopFailure",{agent_id:"child",error:"child error"}).result;await h.flush();
  assert(h.events.some(e=>e.type==="task"&&e.task.id==="child"&&e.task.state==="failed"));
  assert(!h.events.some(e=>e.type==="activity"&&e.value.kind==="failed"));
});

test("Claude streams ordered blocks, native tool arguments and duration without regrouping on completion",async()=>{
  const h=await modHarness();
  async function* chunks(){yield{kind:"text",index:0,text:"before"};yield{kind:"thinking",index:1,text:"thought"};yield{kind:"tool",index:2,id:"native-tool",name:"Bash"};yield{kind:"input",index:2,json:'{"command":"echo hello"}'};yield{kind:"stop"};return{};}
  const stream=h.invoke("turn.step",{turnId:"turn",index:0},()=>{const stream=chunks();stream.result=Promise.resolve({});return stream;}).result;
  for await(const _ of stream){}
  await h.invoke("tool.call",{tool:"Bash",tool_use_id:"native-tool",command:"echo hello"},async event=>{
    await h.invoke("classic.PreToolUse",event).result;
    return{result:{stdout:"hello"}};
  }).result;
  await h.invoke("turn.complete",{turnId:"turn",reason:"answer",durationMs:12450,usage:{inputTokens:20,outputTokens:5}}).result;
  await h.flush();
  const items=new Map();for(const event of h.events)if(event.type==="upsert")items.set(event.item.id,event.item);
  assert.deepEqual([...items.values()].map(item=>item.kind),["assistant","thinking","tool"]);
  const tool=items.get("native-tool");assert.equal(tool.data.input.command,"echo hello");assert.equal(tool.text,"hello");assert.equal(tool.status,"completed");
  assert.equal(h.events.filter(event=>event.type==="turn").at(-1).value.durationMs,12450);
  assert.equal(h.events.filter(event=>event.type==="items").length,1,"completion replaced the ordered live transcript");
});

test("Claude history keeps interleaved blocks and plain string tool results readable",async()=>{
  const h=await modHarness([{role:"assistant",content:[{type:"text",text:"before"},{type:"thinking",thinking:"thought"},{type:"tool_use",id:"native",name:"Bash",input:{command:"echo one"}}]},{role:"user",content:[{type:"tool_result",tool_use_id:"native",content:"native output"}]},{role:"assistant",content:"after"}]);
  await h.flush();const items=h.events.find(event=>event.type==="items").items;
  assert.deepEqual(items.map(item=>item.kind),["assistant","thinking","tool","assistant"]);assert.equal(items[2].text,"native output");
});

test("Claude only native task notifications complete an observed background task",async()=>{
  const h=await modHarness();
  await h.invoke("tool.call",{tool:"Bash",tool_use_id:"bg",command:"sleep",run_in_background:true},async()=>({result:{backgroundTaskId:"job"}})).result;
  const prompt="<task-notification><task-id>job</task-id><status>completed</status><summary>Done</summary></task-notification>";
  await h.invoke("classic.UserPromptSubmit",{prompt,source:"user"}).result;await h.flush();
  assert.equal(h.events.filter(e=>e.type==="task").at(-1).task.state,"running");
  await h.invoke("prompt.submit",{text:prompt,origin:{kind:"task-notification"}}).result;
  await h.invoke("classic.UserPromptSubmit",{prompt}).result;await h.flush();
  const task=h.events.filter(e=>e.type==="task").at(-1).task;
  assert.equal(task.state,"completed");assert.equal(task.message,"Done");assert(task.data.completedAtMs);
});

test("Claude background work survives parent completion and session replacement removes old content",async()=>{
  const h=await modHarness();
  await h.invoke("tool.call",{tool:"Bash",tool_use_id:"shell",command:"long command",run_in_background:true},async()=>({result:{backgroundTaskId:"job-1",stdout:"started"}})).result;
  await h.invoke("turn.complete",{turnId:"turn",reason:"answer",durationMs:500}).result;await h.flush();
  assert.equal(h.events.filter(event=>event.type==="task"&&event.task.kind==="shell").at(-1).task.state,"running");
  await h.invoke("tool.call",{tool:"TaskOutput",tool_use_id:"read-job",task_id:"job-1"},async()=>({result:{status:"completed",stdout:"finished"}})).result;await h.flush();
  const task=h.events.filter(event=>event.type==="task"&&event.task.kind==="shell").at(-1).task;
  assert.equal(task.state,"completed");assert.equal(task.message,"finished");assert(task.data.completedAtMs);
  h.$.session.id=async()=>"replacement";
  await h.invoke("turn.start",{turnId:"new-turn"}).result;
  await h.flush();await h.flush();
  const ready=h.events.findLastIndex(event=>event.type==="ready");
  assert(ready>=0);assert.deepEqual(h.events.slice(ready).find(event=>event.type==="items").items,[]);
});
