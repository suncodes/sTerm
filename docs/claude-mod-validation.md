# Claude Code Mod 双端共享验证记录

验证日期：2026-11-27。

## 结论

本机 Claude Code 2.1.322 的 Mod 可以在保留原生交互终端的同时，把会话和流式内容交给网页展示，并接受网页输入、模型切换、审批和停止指令。终端与网页操作的是同一 Claude 进程、同一会话，运行期间两侧都可以提交输入，由 Claude 安排执行。

本次完成的是独立的可行性实验，尚未接入 sTerm 正式网页。无需用户登录、提供 API Key 或手动操作；没有使用官方云端远程控制。

## 验证环境与边界

- Windows 原生 ConPTY 启动真正的 Claude Code 交互终端，临时通过 --plugin-dir 加载实验 Mod。
- Google Chrome 151.0.7922.109 无头实例运行独立测试页面，通过 CDP 驱动页面按钮并检查 DOM。
- 本机一个 HTTP 服务同时承接 Mod 事件、网页指令与模拟 Anthropic Messages API。
- 模型鉴权使用占位 API Key；模型回复、工具调用请求及分块延迟由本地模拟服务生成。工具仍由真实 Claude Code 执行。
- 配置、工作目录、浏览器 profile、日志均隔离在项目内的任务临时目录；未修改用户 Claude 配置，也未调用真实付费模型服务。
- 最后一次完整验证会话 ID：1221b505-229c-4a5e-badb-f88b1dd0cb89。
- 最后一次完整验证记录：151 个事件、11 次模拟模型请求，页面记录的错误为 0。

## 实测结果

| 能力 | 结果 | 核验方式 |
| --- | --- | --- |
| 保留原生终端 | 通过 | 原生 TUI、输入框、命令和审批弹窗持续工作 |
| 网页共享同一会话 | 通过 | 对比原生会话 ID、网页显示与 Mod 历史快照 |
| 实时输出 | 通过 | 回复未完成时，Chrome DOM 已出现首个文本块 |
| 网页发送输入 | 通过 | 页面发送的文本进入原生会话历史，并在原生 TUI 显示及执行 |
| 运行期间双端输入 | 通过 | 模拟回复暂停时，网页与终端分别提交；同一历史各出现一次并继续执行 |
| 网页切换模型 | 通过 | haiku 别名切换后，原生状态、网页事件及后续模型请求的 model 一致改变 |
| 网页批准工具 | 通过 | 原生弹窗同时存在；网页批准后，真实 Write 工具写入隔离文件 |
| 终端批准工具 | 通过 | 网页桥接持续连接时，在原生审批弹窗按 Enter，工具继续执行 |
| 网页拒绝工具 | 通过 | 拒绝后目标文件未创建，会话继续完成 |
| 审批迟到回复 | 单场景通过 | 终端已批准后，网页回复旧 requestId；没有替下一次审批作答 |
| 网页停止 | 通过 | 当前 turn 的 isAborted 为 true，原生回合结束 |
| 当前会话进入 Plan | 通过 | 网页执行 plan 后，下一次原生输入的 permission_mode 为 plan |
| 任意权限模式切换 | 尚未完整验证 | 设置 permissionMode=acceptEdits 只改变默认值；当前会话仍为 default |
| 网页刷新恢复 | 通过 | 刷新后仍连接同一会话，重新取得此前的输出 |
| 多会话切换、历史会话恢复 | 未验证 | 本轮只验证一个活跃会话 |

“运行期间双端输入”表示两边都能提交到同一会话，不表示两条用户指令会并行执行，也不保证立即打断当前工具步骤。

## 实际使用的 Mod 接口

以运行时为安装版生成的类型声明为准，不能直接照搬最新版在线文档：

| 用途 | 本版已验证接口 |
| --- | --- |
| 会话与历史 | $.session.id()、$.session.model()、$.session.messages() |
| 流式内容 | turn.step 包装 next(e)，读取 chunk 后原样 yield，最后返回 stream.result |
| 网页输入 | $.prompt.submit({ text, asUser: true, wait: false }) |
| 模型切换 | $.config.set({ key: "model", value: "haiku" }) |
| 模型状态同步 | classic.PostModelSwitch 的 to_model |
| 停止 | 保存 turn.start 的 turnId，再调用 $.turn.abort({ turnId }) |
| 工具审批 | classic.PermissionRequest，返回 decision.behavior=allow/deny |
| 当前进入 Plan | $.command.run({ command: "plan" }) |
| 当前权限取证 | classic.UserPromptSubmit 的 permission_mode |

本版不支持在线新版文档中的 $.session.info()；历史读取也不能使用新版的 messages({ last: ... }) 参数。模型别名实际解析出的具体模型 ID 属于本机版本行为，不代表真实网关支持该模型。

Mod 中再次调用同类接口可能跳过本 Mod 对应的事件处理；模型同步应观察实际切换事件或重新读取状态，不能只依赖 config.set hook 自身回调。

## 对 sTerm 的接入建议

连接关系为：原生 Claude Code + Mod ↔ sTerm 本地统一服务 ↔ 网页 Agent 界面。

实验中只有本地服务监听端口，Mod 主动向它发送事件并领取指令。每个 Claude 实例不需要各自对外开放监听端口。接入多个实例时，由统一服务按 sessionId 路由；这一多实例路由尚未实测。

建议先实现文本流、历史、发送、停止、模型和审批。权限界面先按本版已验证能力提供 Plan，不应把“默认权限设置”显示成“当前权限已切换”。

产品实现仍需处理：

- 每个审批绑定会话、turn 与 requestId，只接受一次有效决定；终端完成审批后，网页待办及时失效。
- 断线重连的事件序号、补齐、重复事件及历史快照合并。
- Mod 事件发送失败或 turn 取消时，不阻塞原生输出；实验中停止曾触发被取消的流式 hook 日志。
- 统一服务鉴权、会话隔离，以及 Mod/CLI 版本能力探测。
- 真实模型网关、自定义模型、多个会话与多个网页客户端的联调。

真实 API Key 是否可用、网关是否兼容、具体模型是否有权限均未在本次实验中验证。本次结果支持“Mod 可作本地共享桥接”，不构成生产功能全部完成的结论。

## 参考

- [Claude Code Mods 概述](https://code.claude.com/docs/en/plugins/mods/overview)
- [创建 Mod](https://code.claude.com/docs/en/plugins/mods/create)
- [Mod API](https://code.claude.com/docs/en/plugins/mods/api)
- [官方类型声明](https://github.com/anthropics/claude-code/blob/main/mods/types/claude-code.d.ts)
