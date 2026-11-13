# sTerm

[English](README.en.md) · [隐私说明](PRIVACY.md) · [贡献指南](CONTRIBUTING.md) · [安全策略](SECURITY.md)

sTerm 是一个面向 Windows 的轻量级本地终端，针对 Claude Code、Codex、Pi 等 AI 编程 Agent 的日常工作流做了优化。它使用 Tauri 2、Rust ConPTY 和 xterm.js，不额外打包浏览器运行时。

> 项目处于早期预览阶段。请在重要会话前自行保存工作成果。

## 功能

| 能力 | 说明 |
| --- | --- |
| 本地终端 | 基于 ConPTY，支持 PowerShell 7、Windows PowerShell 和 cmd |
| 多标签与工作区 | 保存、分组、置顶工作区；从侧栏新建对应目录的会话 |
| Agent 辅助 | 识别 Claude、Codex、Pi 的会话状态；从工作区查看可恢复的历史会话 |
| 高效操作 | 搜索、复制粘贴、字号调整、侧栏折叠、快捷键和右键菜单 |
| 可选悬浮窗 | 在主窗口最小化后展示当前 Agent 会话状态；拖至屏幕边缘可收起为状态点条 |

## 系统要求

- Windows 10 1809（Build 17763）或更高版本，x64
- WebView2 Runtime（Windows 10/11 通常已提供）
- 从源码构建时：Node.js 20+、Python 3.8+、Rust stable（MSVC 工具链）

## 安装与校验

发布版本会同时提供以下文件：

- sTerm_<version>_x64-setup.exe：推荐的一键安装包
- sTerm-portable.zip：免安装便携版
- SHA256SUMS.txt：两个发布文件的 SHA-256 校验值

请从 [GitHub Releases](https://github.com/suncodes/sTerm/releases) 下载，并在运行前校验哈希值：

~~~powershell
Get-FileHash ".\sTerm_<version>_x64-setup.exe" -Algorithm SHA256
~~~

便携版解压后必须保留同目录的 sTerm.exe、conpty.dll、OpenConsole.exe、许可证文件和 agent-integration 目录。

## 从源码运行

~~~powershell
npm ci
npm test
cargo test --manifest-path .\src-tauri\Cargo.toml
npm run tauri dev
~~~

构建 NSIS 安装包：

~~~powershell
npm run tauri build
~~~

构建便携 ZIP：

~~~powershell
.\scripts\package.ps1
~~~

安装包输出位于 src-tauri/target/release/bundle/nsis/，便携包输出位于 src-tauri/target/release/。

## 发布新版本

package.json 是唯一需要人工指定版本号的来源。不要分别修改 Cargo、Tauri 或 lock 文件；使用下列命令会同步所有派生元数据，并在最后执行一致性校验：

~~~powershell
npm run version:set -- 0.1.4
npm run version:check
~~~

完成代码和 CHANGELOG.md 后，先将它们提交，使工作区保持干净；随后一条命令即可同步版本、运行校验、创建提交与标签，并原子推送 `main` 和标签：

~~~powershell
npm run release -- 0.1.4
~~~

脚本会要求输入 `v0.1.4` 确认；非交互场景可显式附加 `-Approve`。如确认要把当前未提交改动全部纳入发布，才使用 `-IncludeWorkingTree`：

~~~powershell
npm run release -- 0.1.4 -IncludeWorkingTree -Approve
~~~

推送标签后，GitHub Actions 会校验标签与版本元数据，并构建安装包和便携包。脚本不会直接上传二进制文件。

## Agent 与 Shell 集成

Claude Code 和 Codex 优先使用原生终端状态协议。sTerm 默认不会修改 PowerShell profile、Claude/Codex 配置或 Pi 配置。

如需更精确地跟踪 PowerShell 当前目录，打开「设置 → 集成」并显式安装 PowerShell 集成。该操作只在 $PROFILE 中添加 sTerm 标记包围的 OSC 7 代码块；移除时也只删除该代码块，修改前保留 .bak 备份。

Pi 的状态扩展及 WSL/SSH 场景说明见 [integrations/README.md](integrations/README.md)。

## 数据与隐私

sTerm 默认不包含遥测或分析上报，也不会记录终端输入或输出到诊断日志。工作区和悬浮窗偏好保存在 %APPDATA%\io.github.suncodes.sterm；诊断日志保存在 %LOCALAPPDATA%\io.github.suncodes.sterm\logs。

完整的数据边界、显式集成操作和清理方式见 [PRIVACY.md](PRIVACY.md)。

## 已知限制

参见 [KNOWN_ISSUES.md](KNOWN_ISSUES.md)。

## 参与贡献

欢迎提交 issue 和 pull request。开始前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，安全问题请遵循 [SECURITY.md](SECURITY.md)。

## 许可证与第三方组件

本项目自有代码使用 [Apache License 2.0](LICENSE)。随发布包分发的 ConPTY 运行时及其他第三方说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
