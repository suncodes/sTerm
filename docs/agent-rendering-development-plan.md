# Agent 内容渲染与交互卡开发计划

依据：agent-rendering-event-audit.md、agent-interactions-development-plan.md 及交互区样式评审。

## 执行计划

1. 数据与顺序：保留原生内容字段、回合时间、用量、通知和后台任务身份，统一实时与历史的内容块。
2. 适配与状态：补 Codex 思考/计划增量和遗漏条目，修复 Claude 分块、关闭语义与等待状态。
3. 交互卡：常规审批、问题、计划和表单在输入区处理，详情主动展开；保留明确的设置确认。
4. 后台与统计：展示有证据的后台工作、独立输出、运行/结束耗时与恢复提示；未知信息明确降级。
5. 验证与文档：补协议、Mod、前端回归，复用可用 Remote Dev，隔离 CLI 与 Google Chrome 验证。

## 范围与验收

- 不将工作台内部事件、认证 RPC、语音/SDP、MCP Apps 等条件能力直接声明为已支持。
- 权限目录、原生队列、命令回执、分享授权和原生请求身份沿用现有实现。
- 实时、完成、刷新、重连顺序一致；工具和后台任务按身份更新，父回合完成不代表后台退出。
- 原生原因、命令、diff、范围为默认可读内容；JSON 作为折叠详情。
- 保留回答草稿和焦点；收起不是拒绝；有原生确认才移除待办。
- 原生耗时优先，估算明确标注；并发耗时不累加成回合时长。
- 仅使用 Chrome；验证脚本要求工作区位于非 C 盘，安装版及用户会话不用于模型测试。

## 进度

| 阶段 | 实现与结果 |
| --- | --- |
| 数据与顺序 | 已实现 Item/Task 结构化字段、Turn/Notice/Metadata、服务端时钟；补齐大小限制和重放语义 |
| 适配与状态 | Codex 19 种条目均有展示或明确占位；思考/计划增量、patch、MCP progress、通知和关闭语义已补。Claude 按 step/block 更新，完成不重排，按原生来源处理后台通知 |
| 交互卡 | 已实现输入区卡片、多待办导航、收起/主动放大、草稿与焦点、Other/secret、表单默认值、可读审批与折叠 JSON |
| 后台与统计 | 已实现子 Agent/命令执行/后台命令区分、独立内容、原生/观察耗时、按需输出、用量和提示；未知状态不声明完成 |
| 验证与文档 | 构建、全量回归、Chrome 协议夹具、真实 CLI 完整联调及后台专项通过；临时目录清理被自动审批审查拒绝，清单如下 |

## 验收范围

- 构建：npm run build。
- 回归：最终 53 项 JavaScript、5 项 Python、102 项 Rust 通过，另 1 项辅助进程测试按设计忽略。结果记录、秘密字段不回显、历史字符串工具结果、完成任务容量回收均有回归覆盖。
- Chrome 协议夹具：实时/完成/刷新顺序、原生耗时、待办不弹窗不抢焦点、草稿/选区、异步卡片展开、表单默认值、手机布局。
- 隔离真实 CLI：Codex 0.170.0、Claude 2.1.322、本地模拟模型、两个 Claude/两个 Codex。完整四会话、Full Access、问答/审批、队列、重试/断线、子任务、结束与重启均通过。
- 后台专项：真实 Claude Bash 在主回复结束后继续运行、原生通知结束、网页按需读取输出；Codex 命令执行输出、退出码与 durationMs 均通过。
- 独立 agent-test Cargo profile 避免覆盖 Remote Dev 正在使用的调试二进制。未执行 git 提交或分支操作。

## 保留的能力边界

- 19 种条目可见不等于每种附件均可播放；音频、文档、远程图片等暂为占位，MCP Apps、语音/SDP 和认证 RPC 不属于当前网页执行能力。
- process/spawn、command/exec 与 MCP event subscription 只有存在本客户端创建的绑定时才应显示；本轮没有新建这些宿主能力，不广播未知 handle。
- Codex 原生命令完成不推断任意后台子孙进程结束；Claude 仅使用已确认的原生任务通知，不从聊天文本猜状态。
- Claude 内部 HTTP 重试可能被原生 guard 隐藏，只展示实际可见的错误/恢复/终态，不伪造次数。
- 归档恢复、回退后未提供的待答目录、完整语音、多媒体、工作台目录/搜索和其他 P2/后置项继续按逐项审计标记，不宣称全部事件完整支持。
- 未使用真实模型凭据或生产网关；公网、真实手机键盘和发行包仍需对应环境验收。

## 临时产物

隔离应用和浏览器的测试目录均位于 `D:/document/workspace/tmp/sTerm/.tmp`。首次后台专项发现 Claude 忽略 TEMP/TMP，将一份输出写入默认 C 盘目录；随后通过 CLAUDE_CODE_TMPDIR 修正，新测试确认输出位于 D 盘。自动审批审查拒绝了清理已核对的本轮目录和该 C 盘输出（仅给出 `blocked by policy`），未绕过限制。残留如下：

`C:/Users/Administrator/AppData/Local/Temp/claude/438fff7765538e47/e3482aff-c7d9-4096-8e4e-df0e262feb0c/tasks/bg23lrqda.output`

本轮 D 盘目录（均相对于上述 `.tmp`）：

```text
agent-mode-488b1d96b2a94f1dbb76374a84a04bb8
agent-mode-fbe43c1859084527ad464f0bdce0a027
agent-mode-57058c21155040a9be6e6d337e2e1f0d
agent-mode-ecf8f68ec36648d69436c23801977245
agent-mode-ce84d6f0280f47c982da20d9949ec0fb
agent-mode-f51b5b8c559e4b10b188658e694dfd02
agent-mode-c1e374471adc4c389f03db7daf9ecc7a
agent-mode-c987bd0da25244ecbd31aa82b6d8513a
agent-mode-29ed873930f742adb9f506aedc6309fe
agent-mode-1355131dd68e4095a58d9a35e32e1fa9
agent-rendering-42bc23dde07444d88161fea96b527f37
agent-rendering-8e490bad4e9644ebb75266b7def36e90
agent-rendering-bcf0fe8513bc4a52a46ae4f2e3795b65
```

本轮创建的隔离应用、CLI、mock 和 Chrome 均已退出；原安装版 PID 11444 保留。Remote Dev watcher 在源码变动时会自动重启，未手动结束用户 Agent。原有 `.tmp` 内容不纳入清理。新增 agent-test profile 的编译缓存留在 src-tauri/target/agent-test，便于后续复测。

## 安全阻断补充（2026-11-27）

- [x] 对齐 Codex 0.170.0 的 misalignment 原生说明与继续确认，区分权限审批、普通失败和终态安全阻断。
- [x] 统一 error/turn-completed 映射，保留实时发现并拒绝旧回合、旧说明的确认。
- [x] 输入区独立卡显示原生错误、完整说明与实际继续要求；勾选默认关闭，刷新和说明更新后重新确认。
- [x] 后端从当前实时材料构造继续请求；并发只分发一次，结果未知不自动重发，明确拒绝要求重新确认。
- [x] 无材料或材料失效时提供共享会话切换，并准确说明原生新建/恢复菜单尚未映射。
- [x] 补充固定事件、适配器 RPC 与 Google Chrome 手机触控测试，包含长说明、矮屏、只读、刷新、撤回和重复操作。
- [ ] 新建/恢复原生 CLI 会话的远程菜单接入：当前 TUI 内部切换 thread 无远程驱动接口，保留为独立功能。

补充验证结果：前端构建、JavaScript/Python 全量检查、Rust 全量检查（107 通过、1 个辅助用例忽略）及最后 33 项 Agent 回归通过。Google Chrome 验证了 360×640、390×450、390×844 的触控、长说明、只读、刷新、旧确认和重复操作。真实 Codex 0.170.0 + 隔离本地 Responses 模拟服务验证了带材料的明确确认继续、无材料的安全停止及网页切换共享会话，未向真实模型服务主动触发安全错误。

实现将恢复材料与校验集中在 codex_block / blocking，复用现有命令回执、身份检查和去重链路；原生回合历史不重复保存恢复授权。Remote Dev 的 Tauri watcher 会随源码自动重载，安装版未替换。本轮独立测试进程已退出；临时 Chrome 调试产物已清理。
