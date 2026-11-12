import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({
  stdin: { contents: 'export * from "./src/overlay/types"; export * from "./src/terminal/agentstate"; export * from "./src/overlay/keepalive";', resolveDir: process.cwd() },
  bundle: true, write: false, format: "esm", platform: "node",
});
const { collectAgentSessions, AgentState, OverlayKeepAlive } = await import(
  "data:text/javascript;base64," + Buffer.from(bundle.outputFiles[0].contents).toString("base64")
);
globalThis.window = { setInterval: () => 1, clearInterval: () => {} };

test("overlay follows shared Agent lifecycle and excludes plain shells", () => {
  let current;
  const agentState = new AgentState(() => { current = collectAgentSessions(sessions, 1); });
  const sessions = [
    { id: 1, title: "项目 A", agentState },
    { id: 2, title: "普通终端", agentState: { agent: null, status: "none" } },
  ];
  assert.deepEqual(collectAgentSessions(sessions, 1), []);
  agentState.markAgentEvent("codex", "start");
  assert.deepEqual(current, [{ id: 1, title: "项目 A", agent: "codex", status: "idle", active: true }]);
  agentState.markAgentEvent("codex", "prompt");
  assert.equal(current[0].status, "running");
  agentState.markAgentEvent("codex", "stop");
  assert.equal(current[0].status, "idle");
  agentState.markExited();
  assert.equal(current[0].status, "exited");
  agentState.reset();
  assert.deepEqual(current, []);
  agentState.markAgent("pi");
  assert.equal(current[0].agent, "pi");
  agentState.dispose();
  assert.deepEqual(current, []);
});

test("overlay tracks title, active tab and closed tabs by stable session id", () => {
  const sessions = [
    { id: 1, title: "A", agentState: { agent: "claude", status: "running" } },
    { id: 2, title: "B", agentState: { agent: "codex", status: "idle" } },
    { id: 3, title: "C", agentState: { agent: "pi", status: "idle" } },
  ];
  const before = collectAgentSessions(sessions, 2);
  sessions[1].title = "保存后的工作区名称";
  const after = collectAgentSessions(sessions, 1);
  assert.deepEqual(after.map(s => s.active), [true, false, false]);
  assert.equal(after[1].title, "保存后的工作区名称");
  assert.equal(before[1].title, "B");
  sessions.splice(0, 1);
  assert.deepEqual(collectAgentSessions(sessions, 2).map(s => s.id), [2, 3]);
  assert.deepEqual(collectAgentSessions([], undefined), []);
});

test("overlay keeps background state processing alive only while enabled", async () => {
  const pending = [];
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
    locks: { request: async (name, options, fn) => {
      assert.equal(name, "sterm-agent-overlay");
      assert.equal(options.mode, "shared");
      const promise = fn();
      pending.push(promise);
      await promise;
    } },
  } });
  const keepAlive = new OverlayKeepAlive();
  keepAlive.setEnabled(false);
  assert.equal(pending.length, 0);
  keepAlive.setEnabled(true);
  keepAlive.setEnabled(true);
  assert.equal(pending.length, 1);
  keepAlive.setEnabled(false);
  await pending[0];
  keepAlive.setEnabled(true);
  assert.equal(pending.length, 2);
  keepAlive.setEnabled(false);
  await pending[1];
  // 快速启停：即使浏览器尚未执行回调，也不会遗留未释放的 Lock。
  navigator.locks.request = async (_name, _options, fn) => {
    await Promise.resolve();
    pending.push(fn());
    await pending.at(-1);
  };
  keepAlive.setEnabled(true);
  keepAlive.setEnabled(false);
  await Promise.resolve();
  await pending.at(-1);
});
