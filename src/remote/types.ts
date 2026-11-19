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
  backspace: "\x7f", delete: "\x1b[3~", shiftTab: "\x1b[Z", home: "\x1b[H", end: "\x1b[F", pageUp: "\x1b[5~", pageDown: "\x1b[6~",
};

export function terminalInput(draft: string, appendEnter: boolean, bracketedPaste = false): string {
  const text = draft.replace(/\r?\n/g, "\r");
  return (bracketedPaste ? `\x1b[200~${text}\x1b[201~` : text) + (appendEnter ? "\r" : "");
}

export interface TerminalModifiers {
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
}

export function terminalKey(key: string, applicationCursorKeys = false, modifiers: TerminalModifiers = {}): string {
  if (key === "shiftTab") return terminalKey("tab", applicationCursorKeys, { ...modifiers, shift: true });
  const value = REMOTE_KEYS[key];
  if (!value) return "";
  const parameter = 1 + Number(Boolean(modifiers.shift)) + Number(Boolean(modifiers.alt)) * 2 + Number(Boolean(modifiers.ctrl)) * 4;
  const modified = parameter !== 1;
  if (["up", "down", "left", "right", "home", "end"].includes(key)) {
    if (modified) return `\x1b[1;${parameter}${value.slice(-1)}`;
    return applicationCursorKeys && ["up", "down", "left", "right"].includes(key) ? value.replace("[", "O") : value;
  }
  if (key === "tab") {
    if (modifiers.shift && parameter === 2) return "\x1b[Z";
    return modified ? `\x1b[27;${parameter};9~` : value;
  }
  if (["pageUp", "pageDown", "delete"].includes(key)) return modified ? value.replace("~", `;${parameter}~`) : value;
  if (key === "backspace") {
    const backspace = modifiers.ctrl ? "\x08" : value;
    return modifiers.alt ? `\x1b${backspace}` : backspace;
  }
  if (key === "enter") return modifiers.alt ? `\x1b${value}` : value;
  if (key === "escape") return modifiers.alt ? `\x1b${value}` : value;
  return value;
}
