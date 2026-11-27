import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TabManager } from "../terminal/tabmanager";
import type { TerminalSession } from "../terminal/session";
import { OverlayKeepAlive } from "../overlay/keepalive";
import { readRemoteDefaults, resolveRemotePermission, type RemoteDefaults, type RemotePolicy } from "./permissions";
import type { RemoteStatus, SharedSession } from "./types";
import type { AgentSession } from "../agent/types";

const DEFAULTS_KEY = "sterm-remote-defaults";

export class RemoteController {
  private readonly keepAlive = new OverlayKeepAlive();
  status: RemoteStatus | null = null;
  defaults = readRemoteDefaults(localStorage.getItem(DEFAULTS_KEY));
  private readonly policies = new Map<TerminalSession, RemotePolicy>();
  private readonly synced = new Map<TerminalSession, string>();
  private readonly listeners = new Set<() => void>();
  private pending = false;
  private syncing: Promise<void> | undefined;
  private stopping = false;
  agents:AgentSession[]=[];
  private readonly agentSynced=new Map<string,string>();
  private revision = 0;

  constructor(private readonly tabs: TabManager) {}

  async init(): Promise<void> {
    await Promise.all([
      listen<{ id: number; generation: number; request: string }>("remote-snapshot-request", event => {
        this.tabs.byId(event.payload.id)?.captureRemote(event.payload.request, event.payload.generation);
      }),
      listen<SharedSession>("remote-control", event => this.tabs.byId(event.payload.id)?.applyRemoteControl(event.payload)),
      listen<{ id: number; generation: number; cols: number; rows: number }>("remote-resize", event => {
        const { id, generation, cols, rows } = event.payload;
        void this.tabs.byId(id)?.applyRemoteSize(generation, cols, rows);
      }),
    ]);
    await this.refresh();
  }

  get sessions(): readonly TerminalSession[] { return this.tabs.sessions; }
  async updateAgents(rows:AgentSession[]):Promise<void>{
    const changed=JSON.stringify(this.agents)!==JSON.stringify(rows);
    this.agents=rows;
    this.keepAlive.setEnabled(Boolean(this.status)||rows.some(row=>row.state!=="exited"));
    for(const session of this.sessions){
      const row=rows.find(row=>row.tabId===session.id&&row.generation===session.remoteInfo().generation);
      session.setAgentRuntime(row?.runtimeId??null);
    }
    for(const id of this.agentSynced.keys())if(!rows.some(row=>row.runtimeId===id))this.agentSynced.delete(id);
    await this.sync();
    if(changed)this.notify();
  }

  policy(session: TerminalSession): RemotePolicy {
    return this.policies.get(session) ?? "inherit";
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }

  async refresh(): Promise<void> {
    if (this.stopping) return;
    const revision = this.revision;
    const status = await invoke<RemoteStatus | null>("remote_status");
    if (revision !== this.revision) return;
    this.status = status;
    this.keepAlive.setEnabled(Boolean(this.status)||this.agents.some(row=>row.state!=="exited"));
    this.notify();
  }

  async setDefaults(defaults: RemoteDefaults): Promise<void> {
    localStorage.setItem(DEFAULTS_KEY, JSON.stringify(defaults));
    this.defaults = defaults;
    await this.sync();
  }

  async setPolicy(session: TerminalSession, policy: RemotePolicy): Promise<void> {
    this.policies.set(session, policy);
    session.resumeRemoteSharing();
    await this.sync();
  }

  sync(): Promise<void> {
    this.pending = true;
    if (!this.syncing) {
      this.syncing = Promise.resolve().then(() => this.synchronize()).finally(() => {
        this.syncing = undefined;
        this.notify();
        if (this.pending) return this.sync();
      });
    }
    return this.syncing;
  }

  private async synchronize(): Promise<void> {
    while (this.pending) {
      this.pending = false;
      for (const session of this.policies.keys()) if (!this.tabs.sessions.includes(session)) this.policies.delete(session);
      for (const session of this.synced.keys()) if (!this.tabs.sessions.includes(session)) this.synced.delete(session);
      if(this.stopping)continue;
      for(const row of this.agents){
        const session=this.tabs.byId(row.tabId);
        if(!session||session.remoteInfo().generation!==row.generation)continue;
        const permission=resolveRemotePermission(this.policy(session),this.defaults);
        const key=JSON.stringify([row.generation,permission.share,permission.input]);
        if(this.agentSynced.get(row.runtimeId)===key&&row.shared===permission.share&&row.allowControl===permission.input)continue;
        await invoke("agent_runtime_share",{runtimeId:row.runtimeId,generation:row.generation,shared:permission.share,allowControl:permission.input});
        row.shared=permission.share;row.allowControl=permission.input;this.agentSynced.set(row.runtimeId,key);
      }
      if (!this.status) continue;
      for (const session of this.tabs.sessions) {
        const policy = this.policy(session);
        const permission = resolveRemotePermission(policy, this.defaults);
        const info = session.remoteInfo();
        const enabled = !session.isAgentMode && permission.share && session.isRunning && !session.isRemotePaused;
        const key = JSON.stringify([info.generation, info.title, info.cwd, info.agent, info.agentStatus, enabled, permission.input]);
        if (this.synced.get(session) === key && session.isRemoteShared === enabled
          && (!enabled || session.isRemoteControllable === permission.input)) continue;
        if (enabled || session.isRemoteShared) {
          try { await session.setRemoteShared(enabled, permission.input); }
          catch(error){if(!session.isAgentMode)throw error;}
        }
        this.synced.set(session, key);
      }
    }
  }

  async start(port: number, lan: boolean): Promise<void> {
    this.revision++;
    this.status = await invoke<RemoteStatus>("remote_start", { port, lan });
    for (const session of this.sessions) {
      if (session.isRemoteShared && !this.policies.has(session)) {
        this.policies.set(session, session.isRemoteControllable ? "control" : "readonly");
      }
    }
    this.synced.clear();
    for (const session of this.sessions) session.resumeRemoteSharing();
    this.keepAlive.setEnabled(true);
    await this.sync();
    await this.refresh();
  }

  async stop(): Promise<void> {
    this.stopping = true;
    this.revision++;
    try {
      await this.syncing?.catch(() => {});
      await invoke("remote_stop");
      this.status = null;
      this.synced.clear();
      for (const session of this.sessions) session.remoteDisabled();
      this.keepAlive.setEnabled(this.agents.some(row=>row.state!=="exited"));
    } finally {
      this.stopping = false;
      this.notify();
    }
  }
}
