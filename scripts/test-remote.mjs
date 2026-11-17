import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { Terminal } = require("@xterm/headless");
const { SerializeAddon } = require("@xterm/addon-serialize");
const bundle = await build({
  stdin: { contents: 'export * from "./src/remote/types"; export * from "./src/remote/pending"; export * from "./src/remote/mirror"; export * from "./src/remote/permissions"; export * from "./src/remote/history";', resolveDir: process.cwd() },
  bundle: true, write: false, format: "esm", platform: "node",
});
const { terminalInput, terminalKey, REMOTE_KEYS, PendingTerminalSequence, TerminalMirror, resolveRemotePermission, readRemoteDefaults, InputHistory, INPUT_HISTORY_KEY } = await import(
  "data:text/javascript;base64," + Buffer.from(bundle.outputFiles[0].contents).toString("base64")
);
const write = (terminal, text) => new Promise(resolve => terminal.write(text, resolve));
const screen = terminal => {
  const buffer = terminal.buffer.active;
  return Array.from({ length: terminal.rows }, (_, index) => buffer.getLine(buffer.baseY + index)?.translateToString(true) ?? "");
};

const historyStorage = () => {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
};

test("input history preserves original Chinese and multiline drafts, deduplicates and searches across sessions", () => {
  const storage = historyStorage();
  const history = new InputHistory(() => storage);
  const text = "  中文命令\n第二行  ";
  history.record(text, "会话甲");
  history.record("Git Status", "Shell");
  history.record(text, "会话乙");
  for (const blank of ["", " ", "\r\n\t"]) history.record(blank, "空白");
  assert.equal(history.entries.length, 2);
  assert.equal(history.entries[0].text, text);
  assert.equal(history.entries[0].sessionTitle, "会话乙");
  assert.equal(history.search("第二行")[0].text, text);
  assert.equal(history.search("会话乙").length, 0);
  assert.equal(history.search("git status")[0].text, "Git Status");
  assert.equal(history.search("missing").length, 0);
  const restored = new InputHistory(() => storage);
  assert.deepEqual(restored.entries, history.entries);
  history.record("Git Status ", "Shell");
  assert.equal(history.entries.length, 3);
});

test("input history bounds storage and persists deletion without touching unrelated settings", () => {
  const storage = historyStorage();
  storage.setItem("unrelated-setting", "keep");
  const history = new InputHistory(() => storage);
  for (let index = 0; index < 105; index++) history.record("command " + index, "Shell");
  assert.equal(history.entries.length, 100);
  assert.equal(history.entries[0].text, "command 104");
  assert.equal(history.entries.at(-1).text, "command 5");
  history.remove("command 104");
  assert.equal(new InputHistory(() => storage).entries.length, 99);
  assert.ok(new InputHistory(() => storage).entries.every(entry => entry.text !== "command 104"));
  history.clear();
  assert.deepEqual(new InputHistory(() => storage).entries, []);
  assert.equal(storage.getItem("unrelated-setting"), "keep");
});

test("input history rejects corrupt or excessive entries while retaining valid records", () => {
  const storage = historyStorage();
  for (const raw of ["broken", "null", "[]", '{"version":2,"entries":[]}', '{"version":1,"entries":{}}']) {
    storage.setItem(INPUT_HISTORY_KEY, raw);
    assert.deepEqual(new InputHistory(() => storage).entries, []);
  }
  const entry = { text: "valid", sessionTitle: "Shell", usedAt: 1 };
  const invalid = [null, false, 12, {}, { ...entry, text: " " }, { ...entry, text: "中".repeat(8192) },
    { ...entry, sessionTitle: null }, { ...entry, sessionTitle: "a".repeat(513) },
    { ...entry, usedAt: -1 }, { ...entry, usedAt: 8640000000000001 }];
  storage.setItem(INPUT_HISTORY_KEY, JSON.stringify({ version: 1, entries: [...invalid, entry,
    { ...entry, sessionTitle: "Latest", usedAt: 2 }] }));
  const history = new InputHistory(() => storage);
  assert.deepEqual(history.entries, [{ ...entry, sessionTitle: "Latest", usedAt: 2 }]);
  history.record("中".repeat(8192), "Shell");
  assert.equal(history.entries.length, 1);
});

test("blocked storage or quota errors keep input history usable in memory", () => {
  const denied = new InputHistory(() => { throw new Error("Storage denied"); });
  assert.equal(denied.persistent, false);
  assert.doesNotThrow(() => denied.record("命令", "会话"));
  denied.reload();
  assert.equal(denied.entries[0].text, "命令");
  const quota = new InputHistory(() => ({ getItem: () => null, setItem: () => { throw new Error("Quota exceeded"); } }));
  quota.record("test", "Shell");
  assert.equal(quota.persistent, false);
  assert.equal(quota.entries[0].text, "test");
  assert.doesNotThrow(() => quota.remove("test"));
  assert.deepEqual(quota.entries, []);
  denied.clear();
  assert.deepEqual(denied.entries, []);
});

test("mobile drafts preserve Chinese and require explicit submit", () => {
  assert.equal(terminalInput("中文测试\n第二行", false), "中文测试\r第二行");
  assert.equal(terminalInput("status", true), "status\r");
  assert.equal(terminalInput("中文\n多行", true, true), "\x1b[200~中文\r多行\x1b[201~\r");
  assert.equal(terminalKey("up", true), "\x1bOA");
  assert.equal(terminalKey("ctrlC", true), "\x03");
  assert.equal(REMOTE_KEYS.ctrlC, "\x03");
});

test("default permissions preserve explicit exceptions and reject malformed storage", () => {
  assert.deepEqual(readRemoteDefaults(null), { share: true, input: false });
  for (const value of ['{"share":true}', '{"share":"yes","input":true}', 'null', 'broken']) {
    assert.deepEqual(readRemoteDefaults(value), { share: true, input: false });
  }
  assert.deepEqual(readRemoteDefaults('{"share":false,"input":true}'), { share: false, input: true });
  for (const share of [false, true]) for (const input of [false, true]) {
    const defaults = { share, input };
    assert.deepEqual(resolveRemotePermission("inherit", defaults), { share, input: share && input });
    assert.deepEqual(resolveRemotePermission("off", defaults), { share: false, input: false });
    assert.deepEqual(resolveRemotePermission("readonly", defaults), { share: true, input: false });
    assert.deepEqual(resolveRemotePermission("control", defaults), { share: true, input: true });
  }
});

test("shortcut modifiers preserve application cursor mode and control sequences", () => {
  assert.equal(terminalKey("left", true), "\x1bOD");
  assert.equal(terminalKey("left", true, true), "\x1b[1;5D");
  assert.equal(terminalKey("home", false, true), "\x1b[1;5H");
  assert.equal(terminalKey("tab", false, true), "\x1b[27;5;9~");
  assert.equal(terminalKey("shiftTab"), "\x1b[Z");
  assert.equal(terminalKey("ctrlD"), "\x04");
  assert.equal(terminalKey("ctrlL"), "\x0c");
  assert.equal(terminalKey("pageDown"), "\x1b[6~");
  assert.equal(terminalKey("unknown"), "");
});

const controllerBundle = await build({
  entryPoints: ["src/remote/controller.ts"], bundle: true, write: false, format: "esm", platform: "node",
  plugins: [{ name: "tauri-fixture", setup(plugin) {
    plugin.onResolve({ filter: /^@tauri-apps\/api\// }, args => ({ path: args.path, namespace: "fixture" }));
    plugin.onLoad({ filter: /.*/, namespace: "fixture" }, () => ({ contents: 'export const invoke = (...args) => globalThis.remoteFixture.invoke(...args); export const listen = async () => () => {};', loader: "js" }));
  } }],
});
const { RemoteController } = await import("data:text/javascript;base64," + Buffer.from(controllerBundle.outputFiles[0].contents).toString("base64"));

test("controller applies inherited permissions to new and restarted sessions and preserves exceptions", async () => {
  const storage = new Map();
  globalThis.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
  let enabled = false;
  globalThis.remoteFixture = { invoke: async command => {
    if (command === "remote_start") enabled = true;
    if (command === "remote_stop") enabled = false;
    return enabled ? { enabled: true, port: 18765, origins: [], sessions: [], clients: 0 } : null;
  } };
  const session = id => ({
    id, generation: 1, isRunning: true, isRemoteShared: false, isRemoteControllable: false, isRemotePaused: false,
    remoteInfo() { return { id: this.id, generation: this.generation, title: "会话 " + this.id }; },
    resumeRemoteSharing() { this.isRemotePaused = false; },
    remoteDisabled() { this.isRemoteShared = false; this.isRemoteControllable = false; },
    async setRemoteShared(share, input) { this.isRemoteShared = share; this.isRemoteControllable = input; },
  });
  const inherited = session(1), exception = session(2);
  const tabs = { sessions: [inherited, exception] };
  const controller = new RemoteController(tabs);
  await controller.start(18765, false);
  assert.equal(inherited.isRemoteShared, true);
  assert.equal(inherited.isRemoteControllable, false);
  await Promise.all([controller.sync(), controller.setDefaults({ share: false, input: false })]);
  assert.equal(inherited.isRemoteShared, false);
  await controller.setDefaults({ share: true, input: false });
  await controller.setPolicy(exception, "readonly");
  await controller.setDefaults({ share: true, input: true });
  assert.equal(inherited.isRemoteControllable, true);
  assert.equal(exception.isRemoteControllable, false);
  const created = session(3);
  tabs.sessions.push(created);
  await controller.sync();
  assert.equal(created.isRemoteControllable, true);
  created.generation++; created.remoteDisabled();
  await controller.sync();
  assert.equal(created.isRemoteShared, true);
  await controller.setDefaults({ share: false, input: true });
  assert.equal(inherited.isRemoteShared, false);
  assert.equal(created.isRemoteShared, false);
  assert.equal(exception.isRemoteShared, true);
  await controller.setPolicy(created, "control");
  assert.equal(created.isRemoteControllable, true);
  created.isRemotePaused = true; created.remoteDisabled();
  await controller.sync();
  assert.equal(created.isRemoteShared, false);
  await controller.setPolicy(created, "control");
  assert.equal(created.isRemoteShared, true);
  await Promise.all([controller.setDefaults({ share: true, input: false }), controller.setDefaults({ share: true, input: true })]);
  assert.equal(inherited.isRemoteControllable, true);
  await controller.stop();
  assert.ok(tabs.sessions.every(item => !item.isRemoteShared));
  assert.deepEqual(new RemoteController(tabs).defaults, { share: true, input: true });
  let finishRefresh;
  globalThis.remoteFixture.invoke = command => {
    if (command === "remote_status") return new Promise(resolve => { finishRefresh = resolve; });
    return Promise.resolve();
  };
  const staleRefresh = controller.refresh();
  await controller.stop();
  finishRefresh({ enabled: true, port: 18765, origins: [], sessions: [], clients: 0 });
  await staleRefresh;
  assert.equal(controller.status, null);
});

test("late joining restores alternate screen and incremental output", async () => {
  const source = new Terminal({ cols: 80, rows: 24, allowProposedApi: true });
  const viewer = new Terminal({ cols: 80, rows: 24, allowProposedApi: true, disableStdin: true });
  const serialize = new SerializeAddon();
  const pending = new PendingTerminalSequence();
  source.loadAddon(serialize);
  const initial = "history\r\n\x1b[?1049h\x1b[2J\x1b[3;4HAgent 正在运行\x1b[6;2H> ";
  await write(source, initial);
  pending.feed(initial);
  await write(viewer, serialize.serialize({ scrollback: 300 }) + pending.snapshot());
  assert.equal(viewer.buffer.active.type, "alternate");
  assert.deepEqual(screen(viewer), screen(source));
  await Promise.all([write(source, "继续处理"), write(viewer, "继续处理")]);
  assert.deepEqual(screen(viewer), screen(source));
  source.dispose(); viewer.dispose();
});

test("snapshots preserve a control sequence split across output frames", async () => {
  for (const [prefix, suffix] of [["\x1b[31", "mred"], ["\x1b]0;partial", " title\x07text"], ["\x1b(", "Bplain"]]) {
    const source = new Terminal({ cols: 80, rows: 24, allowProposedApi: true });
    const viewer = new Terminal({ cols: 80, rows: 24, allowProposedApi: true, disableStdin: true });
    const serialize = new SerializeAddon(); source.loadAddon(serialize);
    const pending = new PendingTerminalSequence();
    await write(source, "before" + prefix); pending.feed("before" + prefix);
    await write(viewer, serialize.serialize() + pending.snapshot());
    await Promise.all([write(source, suffix), write(viewer, suffix)]);
    assert.deepEqual(screen(viewer), screen(source));
    pending.feed(suffix); assert.equal(pending.snapshot(), "");
    source.dispose(); viewer.dispose();
  }
});

test("mirror terminals never send device-query responses back to the PTY", async () => {
  const source = new Terminal({ cols: 80, rows: 24 });
  const viewer = new Terminal({ cols: 80, rows: 24, disableStdin: true });
  const sourceReplies = [];
  const viewerReplies = [];
  source.onData(data => sourceReplies.push(data)); viewer.onData(data => viewerReplies.push(data));
  const queries = "\x1b[6n\x1b[c";
  await Promise.all([write(source, queries), write(viewer, queries)]);
  assert.ok(sourceReplies.length > 0);
  assert.equal(viewerReplies.length, 0);
  source.dispose(); viewer.dispose();
});

test("switching sessions discards queued old output and waits before resetting", async () => {
  const callbacks = [];
  let content = "";
  let acknowledgements = 0;
  const mirror = new TerminalMirror({
    reset() { content = ""; },
    write(data, callback) { callbacks.push(() => { content += data; callback(); }); },
  }, () => {});
  const frame = (data, snapshot = false) => ({ data, snapshot, cols: 80, rows: 24 });
  mirror.write(frame("old snapshot", true), () => { acknowledgements++; });
  await Promise.resolve();
  mirror.write(frame("stale output"), () => { acknowledgements++; });
  mirror.invalidate();
  mirror.write(frame("new snapshot", true), () => { acknowledgements++; });
  mirror.write(frame(" + delta"), () => { acknowledgements++; });
  callbacks.shift()();
  for (let index = 0; index < 12; index++) await Promise.resolve();
  callbacks.shift()();
  for (let index = 0; index < 12; index++) await Promise.resolve();
  callbacks.shift()();
  await mirror.flush();
  assert.equal(content, "new snapshot + delta");
  assert.equal(acknowledgements, 2);
});
