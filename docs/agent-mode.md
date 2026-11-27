# Agent 模式

日期：2026-11-27。首版验证版本为 Claude Code 2.1.322、Codex CLI 0.170.0；当前只接入 Windows 原生 CLI。Pi 继续使用原有终端模式。

## 使用

1. 点击电脑端顶栏「Agent」，填写工作目录并启动 Claude 或 Codex。可填写原生历史会话 ID 恢复；已在本实例运行的同一会话不能重复启动。
2. 在面板为目标会话选择「分享 · 只读」或「分享 · 允许操作」。新会话默认不分享。
3. 点击「开启本机网页访问并复制链接」，在本机浏览器打开。手机访问时，先在「设置 → 远程访问」开启局域网或已有公网连接并配对，再切换网页的「Agent」入口；本机 loopback 链接不能供手机直接访问。
4. 网页可切换已分享的会话，查看消息/工具、流式内容和待审批。允许操作后可以输入、停止、切换可用模型和处理审批。终端始终能输入，无需交接控制。
5. 关闭远程访问或撤销浏览器授权会取消尚未分发的网页操作，原生 Agent 继续运行。关闭终端标签或退出 sTerm 才结束对应原生进程。

普通 shell 中自行启动的 CLI 不会自动接入结构化页面；请使用 Agent 入口启动或恢复。会话接入只改变本次启动参数，不要求登录官方云端远程功能，也不替代 CLI 自己的 API Key / 模型网关配置。

## 能力

| 能力 | Claude Code | Codex |
| --- | --- | --- |
| 同一实例的原生终端与网页双向操作 | 支持 | 支持，同一 app-server / thread |
| 文本、流式、历史和工具 | 支持，有限快照 | 支持，有限快照 |
| 空闲时切模型 | 使用 CLI 提供的配置选项 | 使用 model/list，等待原生设置通知 |
| 当前回合停止 | 绑定 turnId | 绑定 turnId |
| 批准/拒绝 | 原生和网页任一侧可处理 | 原生和网页任一侧可处理 |
| 权限操作 | 进入 Plan；当前模式从原生 prompt hook 更新 | 只读 / 工作区写入，按需审批；关闭网络权限 |
| 运行中追加 | 由原生排队语义处理 | 由原生 turn/start 处理 |
| 网页刷新/断线补齐 | 快照与游标恢复 | 快照与游标恢复，Adapter 重连同 thread |
| 图片、附件、AskUserQuestion、自定义 UI | 暂不支持，回原生终端操作 | 暂不支持，回原生终端操作 |

设置动作携带设置修订号；终端或其他网页先修改后，旧请求会被拒绝。停止不会承诺清空 Agent 的原生输入队列。审批回执表示原生请求已结束，不能保证并发响应中是哪一侧最终决定；请以工具实际结果为准。

网页宽度不会调整原生 PTY。360/390/768/1440 CSS px 已检查，小屏通过横向会话栏切换。原终端网页的接管/尺寸同步行为保持原规则；同一 Agent 运行实例禁止再使用网页原始终端控制。

## 本地连接与数据

sTerm 仍使用一个对外网页服务，复用配对 Cookie、访问码、Origin 校验和已有隧道。Claude 的 Mod 共用一个随机 loopback HTTP 桥；多个 Codex thread 共用 sTerm 自己创建的一个认证 loopback app-server。浏览器不直接连接这些内部端口。

Claude 的运行资源生成在应用数据目录 agent-runtime/claude/<runtimeId>，通过 --plugin-dir 只在本次进程加载。桥接令牌通过进程环境传入，生成的 Mod 代码只含随机环境变量名；网页、诊断日志和模型 API 都不接收该令牌。Codex app-server 只在本机监听，命令行仅含令牌摘要，原文通过原生 TUI 的环境变量传入。

公共快照使用 protocolVersion=1、seq、runtimeId、原生 sessionId/threadId 及终端 generation。消息快照有条数和字节上限，超长工具结果截断；事件回放窗口不足会重新获取快照。消息保留在内存，Agent 自己的原生历史仍按其配置保存。草稿保存在当前浏览器标签页的 sessionStorage，切换会话或刷新可恢复，发送成功后清除。

网页命令通过 commandId 去重；结果不明时只查询回执，不自动重发。取消分享、降为只读、撤销设备、会话替换后，已拉取但未分发的旧操作仍会拒绝。

## 验证和边界

本轮使用独立的 sTerm-Agent-Dev.exe、不同应用标识、项目 .tmp/ 内的应用目录、CLI 配置、WebView 和 Chrome profile；原安装版 D:/software/install/sTerm/sTerm.exe 保持运行。模型侧使用本地 Anthropic / Responses SSE 模拟服务；原生 CLI、ConPTY、工具执行、审批以及网页均为真实链路。

已验证两个 Claude 和两个 Codex 路由、双方输入与审批、模型请求、Codex sandbox 变化、网页停止、Chrome 离线时原生继续运行、重连、只读拒绝、草稿恢复、设备撤销和响应式布局。协议与已有终端回归分别由 Rust 和前端测试覆盖。

当前仍有这些限制：

- CLI 版本被固定为上述已验证版本；其他版本会提示不兼容，原普通终端仍可使用。
- Codex 的共享连接和 thread/settings/update 为实验接口，升级 CLI 后需重新验证。
- 原生内部 /new 或切换到另一个 Codex thread 时，网页继续绑定启动时的 thread；新建/恢复请从 sTerm Agent 入口进行。
- Claude 当前模式在用户提交 prompt 后才能得到可靠通知；不会用默认配置冒充当前权限。Plan 的退出和其他权限模式请在原生端操作。
- 应用和 CLI 的首次信任/登录/安全对话框仍需在原生终端处理。API Key 和兼容网关能力由相应 CLI 决定。
- 本地模拟验证不包含真实 API Key/自定义模型网关、实际手机软键盘、公网链路或发行安装包验收。

源码检查：

~~~powershell
npm run build
npm test
cargo test --offline --manifest-path "src-tauri/Cargo.toml" -- --test-threads=1
~~~

安装版正在运行时，不要直接使用其数据目录测试。已有 npm run dev:remote 只隔离 sTerm 应用数据，Agent CLI 仍沿用用户自己的配置；需要完全独立验证时同时隔离 CODEX_HOME、CLAUDE_CONFIG_DIR 和临时目录。

可重复的完整隔离验证使用 PowerShell 7、已安装的上述 CLI、Google Chrome 和已有 Python websocket-client：

~~~powershell
pwsh -NoProfile -File "scripts/test-agent-mode.ps1"
~~~

脚本创建项目 .tmp/ 下的独立配置、模型模拟服务及 Chrome profile，构建自己的 sTerm-Agent-Dev.exe，验证后退出临时进程并清理目录。不读真实 API Key，不更新已安装 sTerm 或 CLI。需要排查时可显式加 -KeepArtifacts 保留产物；脚本会输出绝对路径。若找不到 Chrome，需通过 -ChromePath 指定 Google Chrome，不会回退 Edge。
