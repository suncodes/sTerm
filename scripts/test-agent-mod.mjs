import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 同一公开 Mod 脚本通过模拟 $ API 驱动，验证请求协议，不复制实现。
async function modHarness() {
  const source = await readFile("integrations/claude-mod/hooks/register.js","utf8");
  const module = await import("data:text/javascript;base64,"+Buffer.from("const CONFIG={url:'http://bridge',};\n"+source+"\n//"+Math.random()).toString("base64"));
  const hooks=new Map(),events=[],timers=[],waiting=[],commands=[];
  const $={
    session:{id:async()=>"session",model:async()=>"mock",messages:async()=>[]},
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
