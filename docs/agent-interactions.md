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
| 系统阻断 | misalignmentPolicyViolation 等最终错误 | refusal 等回合结果 | 单独状态，普通输入停止；在原生允许的界面恢复 |
| 启动、信任、登录、更新及原生历史选择器 | CLI 自身界面 | CLI 自身界面 | 仍由电脑端处理 |

request_user_input_async 不是全部用户交互的总入口，也不能用普通输入替代阻塞 RPC 的答案。异步问题的答案通过原生 send_user_message_question_reply 格式保留 questionItemId，防止另一端的问题卡无法关闭。

本机 0.170.0 的 send_message_to_user_async 实验开关实际暴露 send_message_to_user_async(message)；它没有结构化 questions。适配器同时支持协议中的结构化异步问题事件，但此分支目前以适配器测试验证，不能把异步通知测试等同于原生结构化问题测试。

## 权限

网页从原生目录读取选项，识别 Read Only、Default、Full Access、可用 Auto-review 以及自定义 profile，保留受管理策略禁止的选项状态。

Full Access 映射为 :danger-full-access、approvalPolicy=never、approvalsReviewer=user，需要明确确认；Default 映射为 :workspace + on-request。Auto-review 只在原生特性与管理限制均允许时出现。设置成功以 thread/settings/updated 为准，RPC 接收成功不会提前覆盖界面。

命令审批按 availableDecisions 展示。原生可能只允许“批准一次 / 记住命令规则 / 取消”，没有独立“拒绝”；网页保留这个差异。规则修改展示原生规则内容。额外权限只回传该请求提供的 network / fileSystem，不扩大范围。

## 网页面板与原生队列

阻塞交互打开桌面对话框或手机底部面板；关闭面板只收起显示，待办入口仍可重新打开。表单与问题草稿按会话和请求保留在内存，原生结束后清理，不因 350 ms 轮询重建而丢失。秘密输入采用密码字段，不保存到 sessionStorage。

Codex Shift+Left 涉及原生异步问题导航和 queued inputs 编辑。Agent 网页直接提供待办和队列入口，不模拟键盘组合。队列可加入、编辑、删除、上移和立即执行，内容来自 thread/queue/*。普通“追加”仍按 turn/start 语义处理。

Claude 运行中追加由原生安排，网页展示已接受的输入；当前公开桥接没有等价的完整队列编辑接口，不显示虚假的增删改序控件。

## 重试、断线与结束

网页连接、Agent 本地连接、模型 API 重试分别表示：

- 网页与 sTerm 断线：显示“重连中”，保留草稿，不自动重发。
- Agent 本地连接断线：会话显示“Agent 已断开”；恢复后补齐快照和待办。
- Codex 公开 error.willRetry：显示“API 重试中”及原生错误；后续输出恢复为运行。
- Codex 无通知的 HTTP 内部重试：无法逐次显示。流中断产生的原生重连通知已实际验证。
- Claude：原生 guard 默认不向第三方 Mod 公开引擎 HTTP 事件；允许公开时可显示 HTTP 错误，无权限时只显示运行和最终失败/中断/阻断。不会绕过 guard 或伪造重试次数。
- 最终失败、中断和系统阻断分别记录，均不会显示为正常完成。普通失败或中断后仍可发送新要求；系统阻断依原生恢复能力处理。

本轮真实 CLI 验证了 Codex 流重连提示，以及 Claude 多次 HTTP 500 后恢复。Claude 恢复测试不代表第三方 Mod 能读取被 guard 隐藏的每次重试。

## 子 Agent 与并发

任务保留 id、parentId、任务说明、模型、状态、回合和消息，内容按 Agent 分开显示。网页“查看子任务”进入对应内容，待答卡片明确标注来自哪个子 Agent；审批回传原生请求 ID，不把子任务当成另一个可任意编辑的主会话。

Claude 使用 agent.list、agent.spawn 和带 agentId 的 turn/tool 事件。Codex 使用 collabAgentToolCall、subAgentActivity 及子 thread 事件。主任务空闲而子任务仍在运行时，网页明确显示；未知和失败状态需要查看，不当作全部完成。

Codex v1 刚创建子线程时，首次落盘前 thread/resume 可能返回空历史错误。适配器保留真实任务状态并重试这类临时错误；内容可读后补齐。该窗口不承诺完整流式内容。已通过真实 Codex 子任务状态/内容，以及 Claude 子任务流式内容验证。

## 验证记录

- 构建：npm run build。
- 回归：npm test，含真实 Mod 脚本模拟、超过 10 秒等待、原生取消、按完整题干提交多选答案。
- Rust：原生决定限制、权限约束、MCP 枚举/范围/URL、抢答、撤权、迟到回复、子任务归属、系统阻断和批次原子性；异步问题身份与另一端作答关闭。
- 独立应用：scripts/test-agent-mode.ps1，本地模拟模型 + 真实 CLI + Google Chrome，包含四会话、真实权限/工具、问题、队列、重试、子任务、断线、设备撤销和三种 Shell。
- 实际网关、真实凭据、手机软键盘、公网及发行安装包仍需独立验收；MCP/计划及结构化异步问题的罕见原生路径当前主要以协议与脚本测试验证。

最终结果：2026-11-27，前端构建、47 项 JavaScript、5 项 Python、97 项 Rust（另 1 项辅助测试忽略）及完整隔离 Chrome 联调通过。最后补测覆盖两种 Agent 的真实子任务、原生异步通知和 Claude 隐藏重试细节时的正常恢复；结构化异步问题另有适配器测试验证身份、去重、断线保留及另一端作答关闭。
