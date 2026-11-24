import type { IDisposable, IEvent, Terminal } from "@xterm/xterm";

export function onTerminalData(terminal: Pick<Terminal, "onData">, listener: (data: string, userInput: boolean) => void): IDisposable {
  // onData 也包含焦点报告和设备回复；xterm 5.5 仅在真实输入前同步触发内部 onUserInput。
  const core = (terminal as { _core?: { coreService?: { onUserInput?: IEvent<void> } } })._core?.coreService;
  let userInput = false;
  const userListener = core?.onUserInput?.(() => { userInput = true; });
  const dataListener = terminal.onData(data => {
    const fromUser = !userListener || userInput;
    userInput = false;
    listener(data, fromUser);
  });
  return { dispose: () => { dataListener.dispose(); userListener?.dispose(); } };
}
