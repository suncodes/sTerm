# Pi 远程访问扩展选型调研

调研日期：2026-11-27。

## 建议

针对 sTerm 的目标，推荐自行编写一个小型 Pi 桥接扩展，把 Pi 的公开 Extension API 接入 sTerm 现有 Rust/Axum 服务。保留原生 TUI，并由 sTerm 网页渲染结构化消息；多个 Pi 进程主动连接同一个服务，无需每个进程监听网络端口。

现有项目值得复用思路，但没有发现一个已核实能直接满足当前全部条件的现成组件：

- 快速体验同一 Pi 实例的手机 Chat：优先考察 pi-anywhere。
- 接近 sTerm 统一服务架构：优先参考 wkqco33/pi-remote-control。
- 双端审批协调：参考 spikat/pi 的 web + commands-whitelist 集成。
- 若愿意直接使用独立完整平台：优先评估 pi-agent-dashboard。
- 暂不把独立平台整体嵌入 sTerm，也不把“读取同一历史后启动新进程”视为共享运行实例。

本轮为文档、源码和 npm 元信息调研，没有安装候选扩展，没有执行其运行测试，没有升级 Pi，没有修改产品代码。因此下文“支持”表示实现或文档证据，不能替代本机集成验证。

## 本机与上游版本

- 本机：@earendil-works/pi-coding-agent 0.86.0，Node.js 24.11.0。
- npm 当前正式版：@earendil-works/pi-coding-agent 1.6.1。
- 旧包 @mariozechner/pi-coding-agent 最新为 0.73.1，已标记迁移至 @earendil-works。
- 本机公开扩展接口已经包含消息事件、发送用户消息、模型选择、停止、工具拦截及带 AbortSignal 的对话框。
- 版本兼容必须以具体安装版为准，不需要为了调研擅自升级现有 CLI。

## 候选对比

| 候选 | 运行方式与已有能力 | 对 sTerm 的主要限制 | 适用结论 |
| --- | --- | --- | --- |
| Feng-H/pi-anywhere，npm 0.6.0 | 原生扩展；同实例终端与手机 Chat；流式消息、模型、思考等级、停止、历史及分支 | 每实例启动 HTTP/WS 服务并自动启动 Cloudflare Tunnel；其他扩展弹窗只显示 waiting_terminal；未提供通用网页审批 | 体验验证优先；不直接作为 sTerm 的传输层 |
| wkqco33/pi-remote-control，npm 0.4.0 | 扩展主动连接统一 Go relay；多会话、文本/thinking 流、steer/followUp、模型、停止、重连回放和网页审批 | 要求 Pi >=1.6.0；工具审批只作用于远程发起的任务；local/web 是二选一；协议只传工具名与状态，不传完整参数和结果 | 最接近统一服务架构，适合作为主要传输参考 |
| BlackBeltTechnology/pi-agent-dashboard，npm 0.11.0 | 原生 bridge + 完整 Web 平台；多会话、模型、流式消息、PromptBus 双端交互、跨端关闭弹窗 | npm 要求 Pi >=1.3.0；范围包含独立服务、React UI、PTY 终端与大量平台功能；部分集成包含包装内部对象，需要逐项审计 | 想直接使用完整产品时优先；整体接入 sTerm 成本较高 |
| spikat/pi 的 @spikat/pi-web，npm 0.1.8 | 多 Pi 进程连接一个本地 HTTPS bridge；结构化事件；与 commands-whitelist/review 配合时支持终端与网页先答生效、双方关闭 | 定位本机同一用户；只监听 loopback；审批仅覆盖显式接入的扩展；当前主入口未实现完整网页模型切换/停止控制 | 双端审批与显式对话协议的重点参考 |
| h8ntome/piwebui 的 @piwebui/pi-web-bridge，npm 0.1.1 | 零运行时依赖；扩展主动连接统一服务；保留 TUI；流式输出、prompt/steer/followUp、模型、停止、重连 | 产品声明固定验证 Pi 1.5.0；传输采用 Unix socket；明确不支持网页回答其他扩展的终端弹窗，Windows 需另行适配验证 | 可参考协议、事件回放和会话代际控制 |
| tyanxie/paimon，npm 2.5.0 | Hub + Edge + Extension；多机器、多实例、模型与 session 管理、网页与移动适配 | 要求 Pi >=1.1.0；当前明确支持 macOS/Linux；session 切换使用 ExtensionRunner 原型补丁 | 当前 Windows sTerm 不优先采用 |
| ygncode/pi-web，npm 0.0.1-beta.38 | Go Web 服务读取 JSONL，网页发起的聊天由各 session 的 RPC worker 执行 | 文档提供 /refresh 把网页写入的消息拉回终端；不能据此证明网页驱动的是正在运行的原生 TUI 实例 | 不符合本轮同一运行实例双控目标 |
| noahsaso/pi-remote，npm 0.3.1 | PTY + xterm.js + Tailscale；终端画面双向传输 | /remote 会保存并重启 Pi 包装进 PTY；网页仍是终端界面；需要处理终端尺寸 | 与原终端共享路线相近，不适合此次结构化 Agent 页面 |

@monopi/web-remote 0.6.1 的公开说明主要覆盖浏览器观看、历史、工具状态和成本，并写明同时仅一个 remote session；没有足够证据将它列为完整双控首选。

npm 包和仓库开发分支可能不同：Dashboard 当前 develop 文档要求服务端 Node 26，而 npm 0.11.0 server 元信息仍写 Node >=22.19.0。评估时必须固定发布版本，不能混用两套要求。

## 为什么建议自写薄扩展

sTerm 已有 Rust/Axum HTTP 与 WebSocket 服务、网页和访问鉴权。候选项目的大部分额外工作，是独立服务、用户界面、隧道和生命周期管理。再嵌入完整平台会重复这些设施，也会引入第二套会话和访问状态。

真正缺少的是一个运行在原生 Pi 内部的适配器：

| 需要的能力 | 可用公开 API |
| --- | --- |
| 消息和流式输出 | message_start、message_update、message_end |
| 工具进度与结果 | tool_execution_start/update/end、tool_result |
| 工作状态 | agent_start、agent_settled、ctx.isIdle() |
| 当前历史与身份 | ctx.sessionManager.getBranch()、getSessionId() |
| 输入和运行中追加 | pi.sendUserMessage，deliverAs=steer/followUp |
| 模型和思考等级 | pi.setModel、pi.setThinkingLevel、model_select、thinking_level_select |
| 停止当前运行 | ctx.abort() |
| 工具权限判断 | tool_call，拒绝时返回 block=true |
| 本地审批框 | ctx.ui.confirm/select，支持 signal 关闭 |
| 生命周期 | session_start、session_shutdown |

结构建议：

原生 Pi TUI + sTerm Pi Extension → sTerm 统一服务 → 网页 Agent 模式。

扩展与服务使用主动建立的双向连接；网页按会话订阅事件和提交结构化命令。Windows 可使用 loopback WebSocket，避免把 Unix socket 假定为跨平台方案。浏览器与 Agent 连接可以由统一服务的不同路由承接；两者身份与权限应分别处理。

本方案不要求 ACP，不需要 Pi 账户登录或官方云端远程功能。Pi 模型侧继续使用当前配置的 API Key/兼容网关；本地共享服务使用 sTerm 自己的连接凭据。模型凭据无需发送给网页。

## 权限与审批是需要补齐的核心

本机 Pi 文档明确没有内置 permission popups，权限通常通过扩展、工具策略或运行环境实现。不能把 Claude 的 default/acceptEdits/plan/bypassPermissions 原样当成 Pi 原生模式。

可在自己的 tool_call gate 中实现以下流程：

1. 为待审批调用建立 requestId，绑定运行实例、会话及 toolCallId。
2. 向网页发审批请求，并同时用 ctx.ui.confirm/select 打开原生终端审批。
3. 扩展内部统一接收两端决定，第一个有效答案生效。
4. 网页先回答时，用 AbortController 关闭本地弹窗；先记录已决定状态，避免关闭弹窗产生的 false 覆盖已批准结果。
5. 终端先回答时，广播 resolved，移除网页待办。
6. 停止、会话切换或超时撤销待审批状态，拒绝重复或迟到的回复。

这里的双端审批只覆盖自己的 gate，或主动接入同一对话协议的扩展。Pi 的 ui_prompt_start/end 只是通知，只有类型与标题，没有可代答句柄，不能通过监听事件回答任意第三方 custom TUI 弹窗。

wkqco33 实现以 remoteTurn 为前提，并在 web 和 local 之间择一审批，不能直接宣称已实现上述两端同时审批。spikat 的集成已经展示“显式共享待办、先答生效、关闭另一端”的模式，值得重点参考。

“只读”“需审批”“自动执行”等模式应作为该策略扩展的模式，明确约束哪些工具；工具级限制不等于操作系统沙箱。

## 会话切换与停止的边界

网页切换到另一活跃 Pi 实例，只需更改订阅目标，不应为此启动第二个进程打开同一 JSONL，也无需改变原终端的当前会话。

如果要求网页直接让某个 TUI 执行 /new、/resume、/fork，需要使用 ExtensionCommandContext 的公开会话操作，或注册专用命令作为入口，另行验证当前版本的调度与重载行为。不要照搬 Paimon 对 ExtensionRunner 的原型补丁。

停止当前回合与清空全部排队消息是两种能力。公开扩展接口能调用 ctx.abort()，但不能据此宣称队列也被清空；相关按钮和能力声明需要按版本实测。

## 最小验证顺序

1. 用隔离配置和当前本机版本加载一次性扩展，不全局安装。
2. 同一个原生 TUI 与 Chrome 页面共享 sessionId，验证首块输出在回复完成前到达页面。
3. 运行中分别从终端与网页提交，验证历史与队列，不以两个进程读取同一历史替代。
4. 验证模型、思考等级与停止；覆盖切换后实际模型请求。
5. 添加统一 tool_call gate，分别测试终端先答、网页先答、拒绝、迟到回复和停止撤销。
6. 再连接第二个 Pi 实例，验证网页切换、断线恢复与 /reload 后无重复连接。

只有验证公开 API 在当前版确有缺口，才评估升级版本或补充适配；本轮不安装、不升级。

## 其他路线

- 官方 stdio RPC 是无界面进程集成模式，适合未来“由 sTerm 托管 Agent”的模式；它不是给一个已运行原生 TUI 自动增加的旁路接口。
- 本机 docs/development.md 的 experimental remote harness 明确只在源码开发环境提供，相关 server/client 实现排除在 npm 包和独立二进制之外，不宜作为本机 0.86.0 的现成分发能力。
- Pi Durable 是独立的长运行 harness 方向，适合新建托管服务，不是对现有 Pi TUI 的即插即用远程扩展。
- 若后续转向 Android 原生 App，可参考 nucleoid/pi-remote：Android + Pi extension + profile 级共享 daemon；这是单独的产品路线，不是现有 sTerm 网页的直接替代组件。

## 来源与复核位置

- [Pi 官方仓库](https://github.com/earendil-works/pi)
- [Pi 官方 Extension API](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- [Pi 官方 RPC](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md)
- [pi-anywhere](https://github.com/Feng-H/pi-anywhere)，核对 commit b24022b446b6ef4da9d95908420878ad7b45e4c3；重点 src/index.ts、src/server.ts。
- [pi-remote-control](https://github.com/wkqco33/pi-remote-control)，核对 commit 627e2eada3e9d4189c971e6760ba2e49a65f88ed；重点 extension/src/index.ts:360、docs/protocol.md。
- [pi-agent-dashboard](https://github.com/BlackBeltTechnology/pi-agent-dashboard)，核对 develop commit bbd7452cd8bb2e07889559c5edea22d93f06cb53；重点 README、packages/extension/src/bridge.ts。
- [spikat/pi-web](https://github.com/spikat/pi/tree/main/web)，核对 commit 6e90ecbc9586e6fd3c324c8e7875158c31680be0；重点 web/README.md、web/bridge.ts、web/index.ts。
- [piwebui bridge](https://github.com/h8ntome/piwebui/tree/main/packages/pi-extension)，核对 commit c6444537bf669c3c2d2004ccba3cd8c7cc5b3f26；重点 README 和 bridge_plan/02-extension-design.md。
- [Paimon](https://github.com/tyanxie/paimon)，核对 commit 597bc638b358e46fe6e6bffa61229c43c8c305bc；重点 README.md、src/extensions/paimon/session_control_patch.ts。
- [ygncode/pi-web](https://github.com/ygncode/pi-web)，重点 README 的 RPC worker 和 /refresh。
- [noahsaso/pi-remote](https://github.com/noahsaso/pi-remote/tree/main/packages/remote)，重点 PTY 架构与重启流程。
- [Android π Remote](https://github.com/nucleoid/pi-remote)。
- [Pi Durable 说明](https://earendil.com/posts/pi-durable/)。

包版本与依赖要求通过 registry.npmjs.org 对应包 latest 元信息核对；版本和开发分支可能继续变化，以上结论仅代表调研时的证据。
