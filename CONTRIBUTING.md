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

## 版本与发布

package.json 是唯一人工维护的版本来源。需要发布新版本时，执行以下命令同步 package-lock.json、Cargo 元数据与 Tauri 配置；不要手工分别修改这些派生文件。

~~~powershell
npm run version:set -- 0.1.4
npm run version:check
~~~

完成代码和 CHANGELOG.md 后，请先提交这些改动并保持工作区干净。随后用以下命令同步版本、运行检查、创建发布提交和标签，并原子推送 main 与标签：

~~~powershell
npm run release -- 0.1.4
~~~

脚本会在提交和推送前要求输入 `v0.1.4` 确认。只有需要将当前所有未提交改动纳入发布时，才显式使用 `-IncludeWorkingTree`。CI 与发布工作流仍会校验版本元数据；标签不一致时发布会失败。

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
