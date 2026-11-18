import type { AgentStatus } from "../terminal/agentstate";

export interface SharedSession {
  id: number;
  generation: number;
  title: string;
  cols: number;
  rows: number;
  allowControl: boolean;
  controller?: string | null;
  epoch?: string | null;
  seq: number;
  cwd?: string | null;
  agent?: string | null;
  agentStatus?: AgentStatus | null;
}

export interface RemoteStatus {
  enabled: boolean;
  port: number;
  origins: string[];
  sessions: SharedSession[];
  clients: number;
  passwordEnabled: boolean;
}

export interface TunnelStatus {
  provider?: "cloudflare" | "ngrok";
  running: boolean;
  connected: boolean;
  publicUrl?: string;
  message: string;
}

export interface TunnelConfig {
  cloudflareUrl: string;
  cloudflareReady: boolean;
  ngrokReady: boolean;
  cloudflareInstalled: boolean;
  ngrokInstalled: boolean;
}

export const REMOTE_KEYS: Record<string, string> = {
  ctrlC: "\x03", escape: "\x1b", tab: "\t", up: "\x1b[A", down: "\x1b[B",
  left: "\x1b[D", right: "\x1b[C", enter: "\r", ctrlD: "\x04", ctrlL: "\x0c",
  shiftTab: "\x1b[Z", home: "\x1b[H", end: "\x1b[F", pageUp: "\x1b[5~", pageDown: "\x1b[6~",
};

export function terminalInput(draft: string, appendEnter: boolean, bracketedPaste = false): string {
  const text = draft.replace(/\r?\n/g, "\r");
  return (bracketedPaste ? `\x1b[200~${text}\x1b[201~` : text) + (appendEnter ? "\r" : "");
}

export function terminalKey(key: string, applicationCursorKeys = false, ctrl = false): string {
  const value = REMOTE_KEYS[key];
  if (!value) return "";
  if (ctrl && ["up", "down", "left", "right", "home", "end"].includes(key)) return `\x1b[1;5${value.slice(-1)}`;
  if (ctrl && key === "tab") return "\x1b[27;5;9~";
  return applicationCursorKeys && ["up", "down", "left", "right"].includes(key) ? value.replace("[", "O") : value;
}
