# WSL / Linux / SSH 的 Agent 状态接入

Claude Code 和 Codex 统一使用原生 OSC 7501。它们不再需要 sTerm 状态 Hook，也不需要远端 Python 辅助脚本；直接在 sTerm 的 WSL 或带 PTY 的 SSH 会话中运行即可。

- Claude Code 要求 2.1.295 或更新版本；sTerm 会响应其协议能力探测。不要设置 CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1，这会禁用 Claude 的状态探测与报告。
- Codex 需要支持 OSC 7501；已用 0.166.0 的默认共享后台模式验证。
- Pi 继续使用官方扩展机制：Windows 本地写事件信箱，WSL / SSH 内的扩展发送 OSC 777。

## 从旧版升级

Windows：打开「设置 → 集成」，选择“安装 / 更新 Agent 集成”后，sTerm 才会清理 Claude / Codex 配置中调用 sTerm-hook.cmd 或 sterm-agent-events.py 的旧状态 Hook，并在本机已安装 Pi 时写入扩展。该操作保留其他 Hook、权限和模型配置；修改前保存同目录 .bak 备份，不为原生协议创建新配置文件。

WSL / Linux / macOS / SSH：将本目录复制到 Agent 所在环境，在该环境执行：

~~~sh
python3 install.py
~~~

默认清理 Claude / Codex 的旧 sTerm 状态 Hook，并安装或升级 Pi 扩展。只处理某一种 Agent 时，可用 --agent claude、--agent codex 或 --agent pi；前两项只做清理，不添加任何 Hook。

需要 Python 3.8+；Pi 仍使用自己的 Node.js。安装器不下载软件、不调用模型服务。修改已有配置前保存首次 .sterm.bak 备份；配置格式错误时停止，不覆盖错误文件。

支持 CODEX_HOME、CLAUDE_CONFIG_DIR、PI_CODING_AGENT_DIR 自定义目录。--home 可指定隔离主目录，此时不读取这些环境变量。

完成后退出并重新启动 Agent。Windows 的显式安装不会访问 WSL 独立配置；WSL / SSH 需各自执行一次安装器。

## 状态效果

启动后绿色表示空闲，提交任务后黄色表示运行中，任务结束后恢复绿色，退出时清除 Agent 状态。原生协议的等待用户（blocked）和错误（error）归入现有空闲状态，不新增颜色或弹窗；Claude 带 id 的后台任务不覆盖主会话状态。

状态消息沿当前终端的 PTY 实时传输，不同步远端历史会话、工作区、标题或模型内容。旧 Claude / Codex 状态 Hook 即使尚未清理，其消息也不会再改变状态点。

可在 sTerm 的 WSL / SSH 终端手工验证接收：

~~~sh
printf '\033]7501;state=idle:app=claude-code\007'
printf '\033]7501;state=working:app=claude-code\007'
printf '\033]7501;state=done:app=claude-code\007'
printf '\033]7501;state=clear\007'
~~~

Codex 可将 app=claude-code 改为 app=codex。每次只执行一行观察；若 shell 每次提示符发送 OSC 7，提示符会清除状态，持续状态请使用真实 Agent 验证。

## 适用边界

- SSH 需要 PTY，单次远程命令可用 ssh -t。
- tmux / screen 等中间终端需要相应协议支持。Claude 在 tmux 内探测的是 tmux，仅开启 allow-passthrough 不能保证原生状态可用。
- Claude 的后台会话不会像前台交互会话一样向终端报告状态。
- 旧版不支持 OSC 7501 的 Claude / Codex 需要更新 CLI；当前版本不再自动安装其状态 Hook。
- 异常断线可能收不到退出消息；退出到 shell 提示符或重新启动标签可清理状态。
