# 已知限制

以下内容描述当前版本的边界，不代表每个环境都会复现。

## WSL 中调用 Windows 版 Agent

当 WSL 会话通过 Windows 互操作层启动 Windows 版 Claude Code 时，中间控制台可能不完整透传 Agent 状态协议。优先在 WSL 中使用 Linux 原生 Agent；若必须使用 Windows 版，请将状态显示视为尽力而为。

## SSH、tmux 与 screen

Agent 状态依赖交互式 PTY 和中间终端对控制序列的正确透传。

- SSH 执行单次远程命令时，可能需要 ssh -t。
- tmux、screen 或网关终端需要支持相应的透传能力。
- 旧版 Claude Code 或 Codex 可能不支持当前的原生状态协议；请先升级 Agent 再排查链路。

状态显示异常不会影响终端本身、工作区数据或 Agent 的正常执行。

## 平台范围

当前发布包只包含 x64 Windows 控制台运行时。ARM64 和其他操作系统尚未提供官方安装包。

## 反馈问题

提交 issue 时请提供 sTerm 版本、Windows 版本、所用 Shell、Agent 版本和最小复现步骤。不要提交访问令牌、私有路径、提示词内容或终端输出中的敏感信息。
