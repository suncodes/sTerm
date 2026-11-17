import { InputHistory } from "./history";

export class HistoryPanel {
  private readonly search: HTMLInputElement;
  private readonly list: HTMLElement;
  private readonly clear: HTMLButtonElement;
  private readonly confirmation: HTMLElement;
  private canRecall = false;

  constructor(private readonly root: HTMLElement, private readonly history: InputHistory, private readonly recall: (text: string) => void) {
    this.search = root.querySelector<HTMLInputElement>("#history-search")!;
    this.list = root.querySelector("#history-list")!;
    this.clear = root.querySelector<HTMLButtonElement>("#history-clear")!;
    this.confirmation = root.querySelector("#history-clear-confirmation")!;
    this.search.oninput = () => this.render();
    this.clear.onclick = () => {
      this.confirmation.hidden = false;
      root.querySelector<HTMLButtonElement>("#history-clear-cancel")!.focus();
    };
    root.querySelector<HTMLButtonElement>("#history-clear-cancel")!.onclick = () => {
      this.confirmation.hidden = true;
      this.clear.focus();
    };
    root.querySelector<HTMLButtonElement>("#history-clear-confirm")!.onclick = () => {
      history.clear();
      this.render();
      this.search.focus();
    };
  }

  setCanRecall(enabled: boolean): void {
    this.canRecall = enabled;
    this.root.querySelectorAll<HTMLButtonElement>(".history-recall").forEach(button => { button.disabled = !enabled; });
    this.root.querySelector<HTMLElement>("#history-recall-hint")!.hidden = enabled;
  }

  render(): void {
    const entries = this.history.search(this.search.value);
    const storageHint = this.root.querySelector<HTMLElement>("#history-storage")!;
    storageHint.hidden = this.history.persistent;
    storageHint.textContent = "历史暂存于当前页面";
    this.clear.disabled = !this.history.entries.length;
    this.confirmation.hidden = true;
    this.list.replaceChildren();
    const empty = this.root.querySelector<HTMLElement>("#history-empty")!;
    empty.hidden = Boolean(entries.length);
    empty.textContent = this.history.entries.length ? "没有匹配的命令" : "暂无历史命令";
    for (const [index, entry] of entries.entries()) {
      const row = document.createElement("div");
      row.className = "history-entry";
      const fill = document.createElement("button");
      fill.type = "button";
      fill.className = "history-recall";
      fill.disabled = !this.canRecall;
      fill.title = entry.text;
      const preview = document.createElement("code");
      preview.className = "history-preview";
      preview.textContent = entry.text;
      fill.append(preview);
      fill.onclick = () => { if (this.canRecall) this.recall(entry.text); };
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "touch-button history-delete";
      remove.textContent = "×";
      remove.title = "删除此条命令";
      remove.setAttribute("aria-label", remove.title);
      remove.onclick = () => {
        this.history.remove(entry.text);
        this.render();
        const buttons = this.list.querySelectorAll<HTMLButtonElement>(".history-delete");
        (buttons[Math.min(index, buttons.length - 1)] ?? this.search).focus();
      };
      row.append(fill, remove);
      this.list.append(row);
    }
  }
}
