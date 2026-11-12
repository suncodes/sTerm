# 隐私说明

## 默认行为

sTerm 是本地桌面应用。正常运行时，sTerm 自身不发起遥测、分析或账号登录请求。运行在终端中的命令和 Agent 可能自行访问网络；这由相应命令或 Agent 决定，不是 sTerm 的遥测行为。

诊断日志仅记录终端生命周期、尺寸和窗口诊断信息，不记录命令输入、终端输出、提示词或模型响应。

## 本地数据

| 数据 | 默认位置 | 用途 |
| --- | --- | --- |
| 工作区与悬浮窗偏好 | %APPDATA%\io.github.suncodes.sterm | 保存用户创建的工作区、文件夹和悬浮窗设置 |
| 诊断日志 | %LOCALAPPDATA%\io.github.suncodes.sterm\logs | 排查启动、窗口和终端生命周期问题；最多保留当前日志和一份轮转日志 |
| Pi 事件信箱 | %LOCALAPPDATA%\sTerm\events | 在应用运行期间转发 Pi 的状态事件；会话结束后自动清理 |
| WebView 本地设置 | 应用 WebView 存储 | 保存界面字号和侧栏宽度等展示偏好 |

退出 sTerm 后，可自行删除上述目录重置本地设置。删除前请先关闭所有 sTerm 进程。

## 显式集成操作

sTerm 默认不改写 PowerShell profile、Claude/Codex 配置或 Pi 配置。

- 在「设置 → 集成」安装 PowerShell 集成时，sTerm 仅向当前 $PROFILE 写入带有 sTerm 标记的 OSC 7 代码块。移除操作只删除该代码块；每次变更前保留同目录 .bak 备份。
- 在同一页面安装/更新 Agent 集成时，Claude/Codex 仅清理旧版 sTerm Hook，Pi 仅在本机已安装 Pi 时写入其 sterm-events.ts 扩展。已有配置修改前同样保留 .bak 备份。
- WSL、Linux、macOS 或 SSH 远端的可选安装器需要由用户在对应环境中手动运行，说明见 [integrations/README.md](integrations/README.md)。

## 数据不会做什么

- 不上传工作区列表、终端输入输出、Agent 会话内容或诊断日志。
- 不读取或修改不属于 sTerm 集成范围的 Shell profile、Agent Hook 或配置字段。
- 不自动安装 CLI、浏览器扩展、驱动或系统服务。
