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
