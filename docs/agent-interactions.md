# Agent 交互清单与适配边界

日期：2026-11-27。依据本机 Codex CLI 0.170.0 的 app-server 类型、本机 Claude Code 2.1.322 Mod 类型，以及隔离 CLI + Google Chrome 验证。本页区分接口实现、实际 CLI 验证和仍需原生端完成的步骤。

## 哪些情况会等待用户

| 类型 | Codex 来源 | Claude 来源 | 网页行为 |
| --- | --- | --- | --- |
| 普通执行审批 | commandExecution / fileChange requestApproval | PermissionRequest | 显示命令/变更和原生允许的决定；不是统一的 true/false |
| 额外权限 | permissions/requestApproval | 请求自身提供的 permission_suggestions | Codex 显示本回合/本会话/拒绝；Claude 仅提供原生建议的 session addRules |
| 阻塞问题 | item/tool/requestUserInput，常见于 Plan | AskUserQuestion | 专用问题面板、单选/自由回答；Claude 支持多选、多题及选项预览 |
| 异步问题 | agentMessage.delivery=async 且包含 questions | 无对应统一接口 | 保留待办入口，不把 Agent 标为阻塞；回答保留问题身份 |
| 异步消息 | send_message_to_user_async 产生的 agentMessage | 普通消息/通知 | 按原生消息展示；没有 questions 时不捏造选择题 |
| 计划确认 | 依原生实际发出的提问或审批请求 | ExitPlanMode PermissionRequest | 独立计划确认类型，展示计划；批准不隐式切换到更宽松权限 |
| MCP 补充信息 | mcpServer/elicitation/request 的 form / URL | Elicitation / ElicitationResult | 表单或外部授权面板，支持 accept/decline/cancel |
| 外部专用验证 | openai/userVerification、未知动态工具 | 未公开可回复接口的原生界面 | 标记需要电脑端完成 |
| 系统阻断 | misalignmentPolicyViolation 等最终错误 | refusal 等回合结果 | 独立阻断卡；Codex 实时提供完整材料时查看并确认继续，否则切换共享会话 |
| 启动、信任、登录、更新及原生历史选择器 | CLI 自身界面 | CLI 自身界面 | 仍由电脑端处理 |

request_user_input_async 不是全部用户交互的总入口，也不能用普通输入替代阻塞 RPC 的答案。异步问题的答案通过原生 send_user_message_question_reply 格式保留 questionItemId，防止另一端的问题卡无法关闭。网页只解析完整且字段、问题身份合法的回答封套：同一 Agent 的原问题存在时合为可展开的“已回答”摘要，原问题不在当前历史时显示引用题目和答案；实时、增量重放与刷新使用同一展示投影，普通包含标签的文本不被吞掉，已知秘密问题的答案不回显。

本机 0.170.0 的 send_message_to_user_async 实验开关实际暴露 send_message_to_user_async(message)；它没有结构化 questions。适配器同时支持协议中的结构化异步问题事件，但此分支目前以适配器测试验证，不能把异步通知测试等同于原生结构化问题测试。

## 权限

网页从原生目录读取选项，识别 Read Only、Default、Full Access、可用 Auto-review 以及自定义 profile，保留受管理策略禁止的选项状态。

Full Access 映射为 :danger-full-access、approvalPolicy=never、approvalsReviewer=user，需要明确确认；Default 映射为 :workspace + on-request。Auto-review 只在原生特性与管理限制均允许时出现。设置成功以 thread/settings/updated 为准，RPC 接收成功不会提前覆盖界面。

命令审批按 availableDecisions 展示。原生可能只允许“批准一次 / 记住命令规则 / 取消”，没有独立“拒绝”；网页保留这个差异。规则修改展示原生规则内容。额外权限只回传该请求提供的 network / fileSystem，不扩大范围。

## 网页面板与原生队列

常规审批、问题、计划和表单显示在输入框上方的卡片，不自动打开遮罩面板，也不抢输入焦点。多项待办可切换、收起；非阻塞问题默认收起，主动展开可回答。长计划、diff 和表单可以主动放大到桌面对话框或手机底部面板。Full Access、结束会话等明确改变权限或生命周期的操作继续要求单独确认。

草稿按 runtime/session/request 保存到内存，切换、收起、放大和轮询时保留选择与输入焦点；秘密输入不进入 sessionStorage。原生确认后关闭待办并留下结果记录，秘密答案不回显；另一端关闭或撤回时只记录“原生请求已处理或撤回”，不猜测批准结果。

消息、输入框、待办卡和安全阻断卡共用居中内容列及断点留白。审批正文优先显示原生英文原因、命令预览、cwd、文件或网络目标、权限范围及有效期；多行或长命令、文件变更正文、完整命令规则和技术 JSON 默认折叠。长期规则仍明确说明后续匹配命令可自动获准，完整规则可展开核对。审批按钮独立于正文滚动，展开长命令不会挤走按钮。矮屏有待办时精简模型条、输入提示和输入框高度，为原因、预览与操作留出空间。网页保留 CLI 的决定和值，但不复制 TUI 的 ANSI 布局。

Codex Shift+Left 涉及原生异步问题导航和 queued inputs 编辑。Agent 网页直接提供待办和队列入口，不模拟键盘组合。队列可加入、编辑、删除、上移和立即执行，内容来自 thread/queue/*。普通“追加”仍按 turn/start 语义处理。

Claude 运行中追加由原生安排，网页展示已接受的输入；当前公开桥接没有等价的完整队列编辑接口，不显示虚假的增删改序控件。

## 重试、断线与结束

网页连接、Agent 本地连接、模型 API 重试分别表示：

- 网页与 sTerm 断线：显示“重连中”，保留草稿，不自动重发。
- Agent 本地连接断线：会话显示“Agent 已断开”；恢复后补齐快照和待办。
- Codex 公开 error.willRetry：显示“API 重试中”及原生错误；后续输出恢复为运行。
- Codex 无通知的 HTTP 内部重试：无法逐次显示。流中断产生的原生重连通知已实际验证。
- Claude：原生 guard 默认不向第三方 Mod 公开引擎 HTTP 事件；允许公开时可显示 HTTP 错误，无权限时只显示运行和最终失败/中断/阻断。不会绕过 guard 或伪造重试次数。
- 最终失败、中断和系统阻断分别记录，均不会显示为正常完成。普通失败或中断后仍可发送新要求；系统阻断使用输入区上方的独立卡片，不映射成权限审批。
- Codex 安全阻断在 `error` 与 `turn/completed` 统一识别。同一回合不带详情的重复终态保留之前的实时发现；显式撤回或更新材料会撤销旧确认，旧回合迟到事件不能覆盖新回合。
- 原生实时提供完整 `detailedExplanation` 和非空、至多 1024 字节的 `steer.message` 时，用户展开说明并明确勾选后，才可提交原生继续要求。请求绑定原生会话、回合和内存中的发现版本，携带与 CLI 相同的 `misalignment_override` 确认信息。
- 普通文字、队列和旧主会话异步问题不能解除阻断。继续操作按命令去重；结果未知保持已提交状态，不自动重发。明确的 RPC 拒绝可重新查看最新说明后确认。
- 网页刷新可以读取当前宿主仍持有的实时发现，但不保存用户勾选。原生后端断线或进程重启后，恢复材料失效，不从历史日志重建。原生日志本身不保存说明、继续指令和 reviewTarget。
- 缺少有效材料时仅显示原生错误与“切换共享会话”；当前接入不支持远程驱动 CLI 的新建/历史恢复菜单，也不把“回到电脑”作为保证可恢复的提示。提高执行权限不解除安全阻断。

本轮真实 CLI 验证了 Codex 流重连提示，以及 Claude 多次 HTTP 500 后恢复。Claude 恢复测试不代表第三方 Mod 能读取被 guard 隐藏的每次重试。

## 子 Agent 与并发

任务保留 id、parentId、任务说明、模型、状态、回合和消息，内容按 Agent 分开显示。网页“查看任务”进入对应内容，区分子 Agent、原生命令执行及确认的后台命令；待答卡片标明来源。审批回传原生请求 ID。

Claude 使用 agent.list、agent.spawn 和带 agentId 的 turn/tool 事件。Codex 使用 collabAgentToolCall、subAgentActivity 及子 thread 事件。输入区活动入口只显示未终结的子 Agent 与已确认的后台命令，主回合空闲不会清除仍在运行的独立任务。未知、暂停、阻断标为待确认；已完成或失败的历史任务退出活动栏，完整记录从“会话选项 → 任务记录”回看，历史失败不会持续触发查看提示。普通前台命令保留在工具时间线及记录中，不额外算作后台工作。

Codex v1 刚创建子线程时，首次落盘前 thread/resume 可能返回空历史错误。适配器保留真实任务状态并重试这类临时错误；内容可读后补齐。该窗口不承诺完整流式内容。已通过真实 Codex 子任务状态/内容，以及 Claude 子任务流式内容验证。

Claude 的 Bash 原生结果提供 backgroundTaskId 时建立独立后台任务；工具调用返回不代表后台退出。完成仅接受原生 task-notification 来源或原生 TaskOutput/TaskStop 结果，不解析用户自行输入的同名文本。原生提供 outputFile 后可以主动读取/刷新输出，接口仅允许已分享会话中已知任务的文件，最多返回末尾 64 KiB。

Codex commandExecution 保留 processId、输出、退出码和原生耗时；其 completed 表示原生执行记录完成，不证明任意操作系统子孙进程退出。未绑定 process/spawn 的连接级 process/outputDelta、process/exited 不向所有会话广播。断开连接的活跃任务降为“状态待确认”，等待原生重新确认。

## 内容顺序、时间和资源

Claude 按 step/block 身份保存文字、思考和工具，完成时原地更新；Codex reasoning 按 summaryIndex/contentIndex 更新，优先展示摘要，不重复拼接摘要和原文。计划文档、计划步骤进度、文件 diff、工具输出和上下文压缩分别渲染。刷新与重连沿用有序内容块。

回合耗时优先用原生 durationMs，其次使用原生起止时间；Claude 运行中的观察时间标注“约”。网页以服务端时钟和浏览器单调计时延续显示，重连不从零开始。运行时的耗时和 Token 合并为紧凑状态，结束后撤下运行栏，将最近回合摘要放在历史末尾。子任务、后台命令与主回合分别计时，不把并发耗时累加。正常完成、失败、中断和阻断均保留已有时间信息。

图片预览支持受限的内嵌 PNG/JPEG/GIF/WebP，及当前已分享条目的本地图片；本地图片接口按 item ID 定位、检查格式、限制 8 MiB，不接受客户端自定义文件路径。音频、文档、远程图片等无法预览的资源显示明确占位。工具 Apps、认证 RPC、SDP/语音和未托管进程仍属于独立能力边界。

运行提示保留原生 warning、配置/兼容性、自动审查、Hook、模型切换、认证恢复和运行环境变化记录。普通新提示短暂显示；警告/错误显示 10 秒并可关闭，记录从“会话选项 → 运行提示记录”回看。初始快照、刷新和相同状态重放不重复弹历史通知，单独的时间戳刷新不算新提示，已关闭的旧错误不随轮询复活。Hook、自动审查、认证恢复等开始/结束更新同一条记录；进行中状态仅在所属主回合或子任务仍活动时展示，完成或主回合结束后退出输入区。重试详情随运行恢复/回合结束收起，真实安全阻断卡和未处理问题沿用原生关闭语义。原生提供 token 数据时显示；缺少数据不推算为零。不支持的动态工具 callback 返回明确失败，避免留下没有响应者的审批卡。

## 验证记录

- 构建：npm run build。
- 回归：npm test，含真实 Mod 脚本模拟、超过 10 秒等待、原生取消、按完整题干提交多选答案。
- Rust：原生决定限制、权限约束、MCP 枚举/范围/URL、抢答、撤权、迟到回复、子任务归属、系统阻断和批次原子性；异步问题身份与另一端作答关闭。
- 独立应用：scripts/test-agent-mode.ps1，本地模拟模型 + 真实 CLI + Google Chrome，包含四会话、真实权限/工具、问题、队列、重试、子任务、断线、设备撤销和三种 Shell。
- 实际网关、真实凭据、手机软键盘、公网及发行安装包仍需独立验收；MCP/计划及结构化异步问题的罕见原生路径当前主要以协议与脚本测试验证。

上一阶段结果：2026-11-27，前端构建、47 项 JavaScript、5 项 Python、97 项 Rust（另 1 项辅助测试忽略）及完整隔离 Chrome 联调通过。本阶段追加内容块、输入区卡片、后台命令与耗时；最终验收记录见 agent-rendering-development-plan.md。
