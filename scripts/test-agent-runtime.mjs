import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({ entryPoints: ["src/agent/state.ts"], bundle: true, write: false, format: "esm", platform: "node" });
const { applyChanges } = await import("data:text/javascript;base64," + Buffer.from(bundle.outputFiles[0].contents).toString("base64"));
const session = () => ({ runtimeId: "r", sessionId: "s", generation: 1, seq: 4, settingsRevision: 2, model: "a", policy: "", state: "running", turnId: "turn", items: [{ id: "a", kind: "assistant", text: "one" }], approvals: [], capabilities: {} });

test("incremental reconnect deduplicates replay, rejects gaps, and preserves previous snapshots", () => {
  const original = session();
  const event = { seq: 5, events: [{ type: "delta", id: "a", delta: " two" }] };
  const next = applyChanges(original, [event, event]);
  assert.equal(next.items[0].text, "one two");
  assert.equal(original.items[0].text, "one");
  assert.equal(applyChanges(original, [{ ...event, seq: 6 }]), null);
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
