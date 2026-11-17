export interface InputHistoryEntry {
  text: string;
  sessionTitle: string;
  usedAt: number;
}

export const INPUT_HISTORY_KEY = "sterm-remote-input-history-v1";
const HISTORY_LIMIT = 100;
const MAX_INPUT_BYTES = 8192;
type HistoryStorage = Pick<Storage, "getItem" | "setItem">;

function validText(text: unknown): text is string {
  return typeof text === "string" && Boolean(text.trim()) && new TextEncoder().encode(text).length <= MAX_INPUT_BYTES;
}

function readEntries(raw: string | null): InputHistoryEntry[] {
  try {
    const data = JSON.parse(raw ?? "null");
    if (data?.version !== 1 || !Array.isArray(data.entries)) return [];
    const entries: InputHistoryEntry[] = data.entries.filter((entry: InputHistoryEntry | null) => entry
      && validText(entry.text) && typeof entry.sessionTitle === "string" && entry.sessionTitle.length <= 512
      && Number.isSafeInteger(entry.usedAt) && entry.usedAt >= 0 && entry.usedAt <= 8640000000000000);
    const seen = new Set<string>();
    return entries.sort((first, second) => second.usedAt - first.usedAt).filter(entry => {
      if (seen.has(entry.text)) return false;
      seen.add(entry.text);
      return true;
    }).slice(0, HISTORY_LIMIT);
  } catch {
    return [];
  }
}

export class InputHistory {
  private items: InputHistoryEntry[] = [];
  persistent = true;

  constructor(private readonly storage: () => HistoryStorage = () => localStorage) {
    this.reload();
  }

  get entries(): readonly InputHistoryEntry[] { return this.items; }

  reload(): void {
    try {
      this.items = readEntries(this.storage().getItem(INPUT_HISTORY_KEY));
      this.persistent = true;
    } catch {
      this.persistent = false;
    }
  }

  record(text: string, sessionTitle: string): void {
    if (!validText(text)) return;
    this.items = [{ text, sessionTitle: sessionTitle.slice(0, 512), usedAt: Date.now() },
      ...this.items.filter(entry => entry.text !== text)].slice(0, HISTORY_LIMIT);
    this.save();
  }

  search(query: string): readonly InputHistoryEntry[] {
    const normalized = query.trim().toLocaleLowerCase();
    return this.items.filter(entry => entry.text.toLocaleLowerCase().includes(normalized));
  }

  remove(text: string): void {
    this.items = this.items.filter(entry => entry.text !== text);
    this.save();
  }

  clear(): void {
    this.items = [];
    this.save();
  }

  private save(): void {
    try {
      this.storage().setItem(INPUT_HISTORY_KEY, JSON.stringify({ version: 1, entries: this.items }));
      this.persistent = true;
    } catch {
      this.persistent = false;
    }
  }
}
