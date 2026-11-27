import type { AgentItem } from "./types";

export function commandText(item: AgentItem): string {
  if (item.kind !== "tool") return "";
  const input = item.data?.input as Record<string, unknown> | undefined;
  const value = item.data?.command ?? input?.command;
  return typeof value === "string" ? value : Array.isArray(value) && value.every(part => typeof part === "string") ? value.join(" ") : "";
}
export function commandDescription(item: AgentItem): string {
  const input = item.data?.input as Record<string, unknown> | undefined;
  const value = input?.description ?? item.data?.description;
  return typeof value === "string" ? value.trim() : "";
}
export function commandOutput(item: AgentItem): string {
  const command = commandText(item), text = item.text || "";
  return command && text.startsWith(command + "\n\n") ? text.slice(command.length + 2) : text === command ? "" : text;
}
export function successfulCommand(item: AgentItem): boolean {
  return Boolean(commandText(item)) && ["completed", "succeeded"].includes(item.status) && (item.data?.exitCode == null || item.data.exitCode === 0);
}
export function processOutcome(item: AgentItem): "running" | "success" | "failed" | "attention" {
  if (["failed", "error", "declined", "blocked"].includes(item.status) || typeof item.data?.exitCode === "number" && item.data.exitCode !== 0) return "failed";
  if (["running", "inProgress", "in_progress", "pending", "waiting", "retrying"].includes(item.status)) return "running";
  return ["completed", "succeeded"].includes(item.status) ? "success" : "attention";
}
export const previewLine = (text: string): string => (text.split(/\r?\n/).find(line => line.trim()) ?? "").replace(/^\s*#{1,6}\s+/, "").replace(/[*`]/g, "").trim();
