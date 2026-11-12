# 贡献指南

感谢对 sTerm 的关注。请保持改动小而聚焦，并在提交前完成与改动相称的验证。

## 开发环境

- Windows 10 1809+，x64
- Node.js 20+
- Python 3.8+
- Rust stable（MSVC 工具链）
- WebView2 Runtime

~~~powershell
npm ci
npm test
cargo test --manifest-path .\src-tauri\Cargo.toml
npm run tauri dev
~~~

## 提交 Pull Request

1. 先查找已有 issue，说明问题、预期和最小复现方式。
2. 一个 PR 只解决一个明确问题；不要混入格式化、依赖升级或无关重构。
3. 保留现有的中文注释语言风格，并补充必要的测试。
4. 在 PR 描述中写明验证命令及结果；涉及 UI 时附上脱敏截图或录屏。
5. 不要提交构建产物、node_modules、本机路径、日志、令牌、私钥或个人 Agent 配置。

## 临时材料

一次性截图、调试脚本、实验输出和本地诊断材料统一放在仓库根目录 `.tmp/`。该目录已被 Git 忽略，不应存放产品源码、测试资产或需要评审的文档。

## 依赖与许可证

新增运行时依赖前，请说明用途、替代方案、许可证和发布包影响。不要为了方便而引入未经审查的二进制文件。

## 安全问题

请不要在公开 issue 中披露可被利用的安全细节。报告方式见 [SECURITY.md](SECURITY.md)。
