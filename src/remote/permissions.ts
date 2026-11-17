export type RemotePolicy = "inherit" | "off" | "readonly" | "control";

export interface RemoteDefaults {
  share: boolean;
  input: boolean;
}

export const DEFAULT_REMOTE_PERMISSIONS: RemoteDefaults = { share: true, input: false };

export function resolveRemotePermission(policy: RemotePolicy, defaults: RemoteDefaults): RemoteDefaults {
  if (policy === "off") return { share: false, input: false };
  if (policy === "readonly") return { share: true, input: false };
  if (policy === "control") return { share: true, input: true };
  return { share: defaults.share, input: defaults.share && defaults.input };
}

export function readRemoteDefaults(value: string | null): RemoteDefaults {
  try {
    const saved: unknown = JSON.parse(value ?? "null");
    if (saved && typeof saved === "object" && "share" in saved && "input" in saved
      && typeof saved.share === "boolean" && typeof saved.input === "boolean") {
      return { share: saved.share, input: saved.input };
    }
  } catch {}
  return { ...DEFAULT_REMOTE_PERMISSIONS };
}
