# Agent 交互与运行状态开发计划

日期：2026-11-27。针对 Codex CLI 0.170.0、Claude Code 2.1.322；详见 [交互清单](agent-interactions.md)。

## 目标与执行顺序

| 阶段 | 开发内容 | 当前结果 |
| --- | --- | --- |
| A 协议与状态 | 带类型和来源的交互、权限目录、原生队列、子任务、错误活动 | 已实现；快照与增量同步，提交和派发均校验请求身份及授权 |
| B Codex | 原生权限预设及自定义目录、完整审批决定、同步问题、异步问题事件、MCP、队列 | 已实现；Read Only / Default / Full Access、Plan 阻塞问答、真实队列已通过 CLI + Chrome |
| C Claude | AskUserQuestion、Plan、MCP、长时间等待及原生取消 | 已实现；多题、多选、自由回答按题干返回，超过 10 秒真实审批仍可回答 |
| D 运行信息 | API 重试/失败/中断/系统阻断、父子任务与内容归属 | 已实现；Codex 公开重试通知已验证，两种 Agent 的真实子任务已验证 |
| E 网页与验证 | 桌面对话框、手机底部面板、草稿、双端竞争、撤权和重连 | 已实现；全量隔离回归通过 |

## 关键设计

- Interaction 区分 approval、question、asyncQuestion、permissions、plan、form、url 和 external，保留原生请求身份、回合与子 Agent 来源。
- 服务端只接受当前原生请求支持的响应；保留 commandId 去重、generation、sessionId、settingsRevision 校验。迟到、撤权、抢答和已结束请求不能重新派发。
- 权限来源为 permissionProfile/list、configRequirements/read 和分页的原生能力目录。设置等待原生通知确认。Full Access 有明确确认；不会自动启用实验能力。
- Codex Queue 使用原生增删改序/执行接口，与普通追加区分。队列项目使用 queueId，避免与命令 id 冲突。
- Claude 等待通过真正的本机 HTTP 长轮询，不再在普通 Promise 内等待 120 秒。注册成功后再等待，避免与后台 flush 竞争；原生取消时清理网页待办。
- 子 Agent 内容使用独立命名空间，审批回复仍回原生请求。主任务空闲不等于全部子任务完成。
- 只呈现原生公开的重试状态。Claude guard 隐藏的 HTTP 事件、Codex 内部无通知的短暂重试不推测次数。
- MCP 平面字段及规范枚举可远程填写；复杂或专用验证界面明确留在电脑端。

## 验证和边界

已通过前端构建、JavaScript/Python 回归、Rust 单元回归，以及独立应用 + 两 Claude + 两 Codex + Google Chrome 的核心交互验证。新增子任务专项已通过，两种 Agent 均读取实际 CLI 事件与内容；最终全量通过 47 项 JavaScript、5 项 Python、97 项 Rust（另 1 项辅助测试忽略），以及完整隔离 Chrome 联调。

测试使用项目 .tmp/ 内隔离配置和本地模拟模型；没有使用真实凭据、实际模型网关、生产环境或发行安装包。已安装 sTerm 保持运行。未执行 git 提交或分支操作。

Codex v1 子线程首次落盘前可能暂时拒绝订阅，现保留任务状态并重试明确的空记录错误；这期间不承诺完整子任务流式内容。异步问题卡依赖原生 questions 事件，本机启用的 send_message_to_user_async 只提供消息，按原生语义显示通知。完整差异见交互清单。
