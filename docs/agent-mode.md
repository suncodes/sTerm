# Agent 模式

日期：2026-11-27。当前验证版本为 Claude Code 2.1.322、Codex CLI 0.170.0，接入 Windows 原生 CLI；Pi 沿用终端模式。页面与入口按 [已确认原型](agent-mode-prototype.md) 调整。

## 使用

1. 在 sTerm 的 PowerShell 7、Windows PowerShell 或 CMD 中照常输入 claude / codex。已启用接入的新终端使用当前进程内的临时包装；不再需要独立启动面板或手填网页会话 ID。
2. 到「设置 → 远程访问 → 会话权限」设置默认分享、默认允许输入及单个会话例外。沿用原有首次默认：远程服务关闭，默认分享开启，默认输入关闭。Agent 可操作时两端同时输入，无需接管。
3. 开启远程访问，通过现有二维码、访问码或配对链接连接。网页切换「终端 / Agent」；手机通过右侧列表切会话，宽屏通过 Tab 或列表切换。
4. 网页显示实际消息、工具、思考摘要和实时输出。模型、权限、审批、停止都有按钮或选择面板，按当前 Agent 的实际能力显示。
5. 「停止生成」只中断当前回合。「更多 → 结束 Agent 会话」需要确认，仅结束对应托管 Agent，保留其 Shell、网页历史和其他会话。「不分享」仅撤销网页访问，电脑继续使用。

「设置 → 集成 → 自动接入终端中的 Claude / Codex」控制后续新建终端是否装载临时包装。已有包装也会在下次启动 Agent 时检查开关；已运行 Agent 不因开关或网页关闭而退出。关闭终端标签或退出独立 sTerm 会结束该标签内的 Agent。

不会为接入写入用户 Shell profile、修改全局 PATH、修改 CLI 的用户配置，或接管在别处已经运行的进程。仍使用 CLI 自己的 API Key / 模型网关配置，无需官方云端远程功能。

## 启动兼容范围

- Claude 普通交互、常用参数及原生 --resume 通过本次 --plugin-dir 加载桥接。
- Codex 普通启动、单个初始 prompt、resume 会话 ID、-m / --model、-s / --sandbox、-a / --ask-for-approval、-C / --cd、--no-alt-screen 可接入。
- Codex 交互式历史选择器、--last、配置覆盖、profile、--remote 等其他参数暂用原生 CLI，不自动共享到 Agent 页面。CLI 内部切换新 thread 的自动跟随仍未支持。
- 版本不兼容或接入失败时提示原因，继续原生 CLI；帮助、版本、非交互命令保留原有行为。某些未支持参数会提示「未接入网页 Agent 模式」。
- 首次信任、登录、权限确认等原生对话框仍在电脑终端处理；不用网页假装已经接入。

## 能力

| 能力 | Claude Code | Codex |
| --- | --- | --- |
| 同一实例的原生终端与网页双向操作 | 支持 | 同一 app-server / thread |
| 文本、实时输出、历史和工具 | 有限快照 | 有限快照 |
| 空闲时切模型 | CLI 提供的选项 | model/list，等待原生设置通知 |
| 当前回合停止 | 绑定 turnId | 绑定 turnId |
| 批准一次 / 拒绝 | 原生和网页任一侧处理 | 原生和网页任一侧处理 |
| 权限操作 | 进入 Plan；计划确认单独处理 | 原生权限目录、Read Only / Default / Full Access、可用 Auto-review 与自定义 profile |
| 运行中追加 | 原生安排，无完整队列编辑接口 | turn/start 追加及原生队列增删改序/执行 |
| 刷新 / 断线补齐 | 快照与游标恢复 | 快照与游标，重连同 thread |
| 结束 Agent | 托管进程退出，Shell 保留 | 托管 TUI 退出，Shell 保留 |
| 交互问题 | AskUserQuestion 多题、多选、自由回答和预览 | 阻塞问题与协议提供的结构化异步问题 |
| MCP 表单与 URL | 类型化交互，复杂界面回电脑端 | 类型化交互，外部专用验证回电脑端 |
| API 重试 / 失败 / 中断 | 显示原生公开信息；guard 可能隐藏 HTTP 细节 | willRetry 与实际终态分开展示 |
| 子 Agent 与并发 | 独立任务状态、来源和内容 | 父子 thread 状态与内容，首次订阅可能延迟 |
| 图片、附件 | 暂不支持 | 暂不支持 |

「Agent 执行权限」与「网页分享权限」分开：Agent 使用只读执行策略时，网页仍可提交要求。模型和权限变更携带设置修订号，冲突会被拒绝并显示原因。

Claude 追加提示只表示原生接受，不保证已经执行；Codex 的队列面板读取真实原生队列。停止当前回合不承诺清空原生输入队列。审批回执以原生请求结果为准，并发时不保证是哪一侧的决定胜出；请以工具结果为准。

## 页面行为

- 灰色主题、连接栏、4 px 控件圆角、手机单会话栏与右侧列表、宽屏 Tab。
- 消息独立排版、Markdown 安全渲染、工具结果折叠；只展示协议提供的思考摘要。
- 待办入口固定在输入区上方；阻塞问题打开手机底部面板或桌面对话框。问题、审批、权限、计划和 MCP 表单分别呈现，答案草稿不因轮询丢失。
- 发送按钮固定 72 × 44 px；多行输入、中文及 Ctrl/Cmd + Enter 发送。
- 网页只读或断线时收起输入区；每会话草稿分别保存，刷新和重连恢复，结果不明的输入不自动重发。
- 网页显示尺寸不会改变电脑 PTY。普通终端模式保留原来的接管、归还和尺寸同步规则。
- 结束会话后内容在当前 sTerm 运行期间保留；关闭标签或同标签启动新 Agent 后由新实例替换。

## 本地通信与数据

sTerm 复用一个对外网页入口、配对 Cookie、访问码、Origin 校验及隧道。内部有一个 Shell 启动桥、一个共用 Claude Mod 桥，以及一个供多个 Codex thread 使用的认证 loopback app-server；浏览器不直接访问这些端口。

启动包装生成在应用数据目录 agent-runtime/shell，通过当前 Shell 的函数或局部 PATH 加载。凭据绑定当前 sTerm 实例、终端标签和代际；退出后失效。直接输入完整 exe 路径、WSL 或 SSH 不经过本地包装。

Claude Mod 位于 agent-runtime/claude/<runtimeId>，本次启动通过 --plugin-dir 加载；桥接令牌通过独立子进程环境传入。Codex 后端使用认证令牌，命令行仅传摘要，原文通过 TUI 子进程环境传入。模型 API Key 不发给网页。

接入时核对原生进程路径及父进程，只托管当前 Shell 创建的 Agent。网页结束 Agent 不按进程名称批量终止，不结束已有安装版或其他终端会话。

快照 protocolVersion=1，使用 seq、runtimeId、原生 sessionId/threadId 和终端 generation。消息有条数及字节上限，大工具结果截断；回放不足则重新取快照。消息在内存，原生历史仍由 CLI 保存。草稿使用当前浏览器标签页的 sessionStorage。

网页命令以 commandId 去重；结果不明时查询回执、不重发输入。取消分享、降权、撤销设备和替换会话后，未分发的操作会被拒绝。

## 隔离验证

使用独立 sTerm-Agent-Dev.exe、不同应用标识、项目 .tmp/ 内的应用数据、CLI 配置、WebView 和 Google Chrome profile。安装版 D:/software/install/sTerm/sTerm.exe 继续运行。

本地 Anthropic / Responses SSE 模拟模型用于可重复验证；CLI、ConPTY、工具执行、审批与网页是实际链路。已检查两 Claude、两 Codex 的自动接入、独立路由、双端输入与审批、模型请求、Codex sandbox、网页停止、断线恢复、草稿恢复、只读拒绝、设备撤销、结束 Agent 保留 Shell 与其他会话、响应式页面不改变 PTY。

2026-11-27 续接验证修复了两项退出问题：生成中“结束 Agent 会话”的确认框不再被状态轮询关闭；Codex TUI 退出或标签关闭后，会取消对应后台活跃回合并释放订阅。已结束会话的网页历史继续保留，但不再作为活跃 Agent 运行。

本轮补齐交互类型、原生权限目录、Codex 队列、重试与终态、子 Agent 内容，详细实现与限制见 [交互清单](agent-interactions.md) 和 [开发计划](agent-interactions-development-plan.md)。新增回归包含 Claude 超过 10 秒的真实等待、选择题按完整题干回答、原生取消、双端竞争及子任务独立归属。

前端和 Rust 检查：

~~~powershell
npm run build
npm test
cargo test --manifest-path "src-tauri/Cargo.toml" --lib -- --test-threads=1
~~~

完整独立联调使用 PowerShell 7、上述已安装 CLI、Google Chrome 和已有 Python websocket-client：

~~~powershell
pwsh -NoProfile -File "scripts/test-agent-mode.ps1" -ChromePath "C:/path/to/Google/Chrome/Application/chrome.exe"
~~~

脚本创建项目 .tmp/ 下的独立配置与模拟服务，构建临时调试版，验证后退出进程并尝试清理。调试时可加 -KeepArtifacts。不会读取真实 API Key、更新已安装 sTerm 或升级 CLI。

## 尚需验收

真实 API Key / 自定义网关、实际手机软键盘、公网连接及发行安装包尚未验收。Codex --remote 与 thread/settings/update 为实验接口，升级 CLI 版本后需重新验证。Claude 当前权限从原生提交事件更新；ExitPlanMode 作为独立计划确认，其他权限模式仍回原生终端。Claude guard 隐藏的重试细节及 Codex 无通知的内部重试不能逐次显示。
