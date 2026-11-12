import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({
  stdin: { contents: 'export * from "./src/terminal/agentevents"; export * from "./src/terminal/agentstate";', resolveDir: process.cwd() },
  bundle: true, write: false, format: "esm", platform: "node",
});
const { AgentOscChannel, parseAgentOsc, parseAgentKind, parseProgramStatus, AgentState } = await import(
  "data:text/javascript;base64," + Buffer.from(bundle.outputFiles[0].contents).toString("base64")
);

globalThis.window = { setInterval: () => 1, clearInterval: () => {} };
const frame = (agent, event, session = "session-1") => "sterm;v=1;agent=" + agent + ";event=" + event + ";session=" + session;
const native = (agent, state, extra = "") => "state=" + state + ":app=" + (agent === "claude" ? "claude-code" : agent) + extra;
const setup = () => {
  const changes = [];
  const state = new AgentState(() => changes.push([state.agent, state.status]));
  const channel = new AgentOscChannel(({ agent, event }) => state.markAgentEvent(agent, event));
  return { state, channel, changes };
};

for (const agent of ["codex", "claude"]) {
  test(agent + " native lifecycle needs no hooks", () => {
    const { state, channel, changes } = setup();
    for (const value of ["idle", "working", "done"]) {
      assert.equal(channel.acceptProgramStatus(native(agent, value)), true);
      assert.equal(channel.hasControl, true);
    }
    channel.acceptProgramStatus("state=clear");
    assert.deepEqual(changes, [[agent, "idle"], [agent, "running"], [agent, "idle"], [null, "none"]]);
    assert.equal(channel.hasControl, false);
    state.dispose();
  });

  test(agent + " waiting or error stops the running indicator", () => {
    const { state, channel } = setup();
    for (const report of [native(agent, "blocked", ":kind=permission"), native(agent, "error", ":msg=ZmFpbGVk")]) {
      channel.acceptProgramStatus(native(agent, "working"));
      channel.acceptProgramStatus(report);
      assert.equal(state.status, "idle");
      assert.equal(state.agent, agent);
      assert.equal(channel.hasControl, true);
    }
    state.dispose();
  });
}

test("Pi extension retains its lifecycle and delayed-session filtering", () => {
  const { state, channel, changes } = setup();
  for (const event of ["start", "prompt", "stop", "end"]) channel.accept(frame("pi", event));
  assert.deepEqual(changes, [["pi", "idle"], ["pi", "running"], ["pi", "idle"], [null, "none"]]);
  channel.accept(frame("pi", "prompt"));
  assert.equal(state.status, "none");
  channel.accept(frame("pi", "start"));
  assert.equal(state.status, "idle");
  state.dispose();
});

test("old Codex and Claude hook messages never drive status", () => {
  const { state, channel } = setup();
  for (const agent of ["codex", "claude"]) {
    for (const event of ["start", "prompt", "stop", "end"]) {
      assert.equal(parseAgentKind(agent + "-" + event), null);
      assert.equal(channel.accept(frame(agent, event)), true);
    }
  }
  assert.equal(state.status, "none");
  channel.acceptProgramStatus(native("claude", "working"));
  for (const agent of ["codex", "claude"]) channel.accept(frame(agent, "end"));
  assert.equal(state.agent, "claude");
  assert.equal(state.status, "running");
  channel.acceptProgramStatus("state=clear");
  channel.accept(frame("claude", "prompt"));
  assert.equal(state.status, "none");
  state.dispose();
});

test("native fields allow metadata and arbitrary order without ambiguous duplicates", () => {
  assert.deepEqual(parseProgramStatus("msg=VGVzdA==:progress=40:app=claude-code:state=working:title=VGl0bGU="),
    { agent: "claude", state: "working", id: null });
  const invalid = [
    "state=working:app=other", "state=idle", "state=unknown:app=codex",
    "state=done:app=codex:app=claude-code", "state=clear:app=other",
    "state=working:app=claude-code:state=done", "state=working:app=codex:bad",
    "state=working:app=codex:id=", "state=done:app=codex:msg=bad\x07",
    "state=done:app=codex:msg=" + "x".repeat(4096),
  ];
  const { state, channel } = setup();
  channel.acceptProgramStatus(native("codex", "working"));
  for (const value of invalid) {
    assert.equal(parseProgramStatus(value), null, value.slice(0, 100));
    assert.equal(channel.acceptProgramStatus(value), false);
  }
  assert.equal(state.status, "running");
  state.dispose();
});

test("background task updates and clears never overwrite the foreground", () => {
  const { state, channel, changes } = setup();
  channel.acceptProgramStatus(native("claude", "working"));
  for (const value of ["done", "idle", "error", "clear"]) {
    channel.acceptProgramStatus(native("claude", value, ":id=agent/task-1"));
  }
  channel.acceptProgramStatus("state=clear:id=agent");
  assert.deepEqual(changes, [["claude", "running"]]);
  channel.acceptProgramStatus(native("codex", "clear"));
  assert.equal(state.status, "running");
  channel.acceptProgramStatus("state=clear");
  assert.equal(state.status, "none");
  state.dispose();
});

test("native clear cannot clear Pi and Pi can take over after native exit", () => {
  const { state, channel } = setup();
  channel.accept(frame("pi", "start"));
  for (const value of ["state=clear", native("codex", "clear"), native("claude", "clear")]) {
    channel.acceptProgramStatus(value);
    assert.equal(state.agent, "pi");
  }
  channel.acceptProgramStatus(native("codex", "working"));
  channel.accept(frame("pi", "end"));
  assert.equal(state.agent, "codex");
  channel.acceptProgramStatus("state=clear");
  channel.accept(frame("pi", "start", "next-session"));
  assert.equal(state.agent, "pi");
  state.dispose();
});

test("shell prompt and restart release ownership and native tabs remain isolated", () => {
  const a = setup(), b = setup();
  a.channel.acceptProgramStatus(native("claude", "working"));
  b.channel.acceptProgramStatus(native("codex", "idle"));
  a.channel.shellPrompt();
  a.state.markShellPrompt();
  assert.equal(a.channel.hasControl, false);
  assert.equal(a.state.status, "none");
  assert.equal(b.state.agent, "codex");
  assert.equal(b.channel.hasControl, true);
  a.channel.accept(frame("pi", "prompt"));
  a.channel.reset();
  a.channel.acceptProgramStatus(native("claude", "done"));
  assert.equal(a.state.agent, "claude");
  a.state.dispose(); b.state.dispose();
});

test("malformed Pi protocol fields cannot alter state", () => {
  const base = frame("pi", "start");
  const invalid = [
    base + ";event=prompt", base.replace("v=1", "v=2"),
    base.replace("agent=pi", "agent=unknown"), base.replace("event=start", "event=unknown"),
    base.replace("session=session-1", "session="), base + ";extra=value",
    base.replace("session-1", "bad/session"), base.replace("session-1", "x".repeat(600)),
  ];
  const events = [];
  const channel = new AgentOscChannel(event => events.push(event));
  for (const value of invalid) {
    assert.equal(parseAgentOsc(value), null);
    channel.accept(value);
  }
  assert.equal(channel.accept("another-app;data=1"), false);
  assert.deepEqual(events, []);
  assert.equal(parseAgentKind("pi-prompt-extra"), null);
  assert.deepEqual(parseAgentKind("pi-prompt"), { agent: "pi", event: "prompt" });
});

test("switching native agents at the same status refreshes the identity", () => {
  const { state, channel, changes } = setup();
  channel.acceptProgramStatus(native("codex", "idle"));
  channel.acceptProgramStatus(native("claude", "idle"));
  assert.deepEqual(changes, [["codex", "idle"], ["claude", "idle"]]);
  state.dispose();
});
