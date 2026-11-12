# Windows 控制台运行时

- 文件版本：1.23.2510.08001；架构：x64。
- 来源：Microsoft VS Code 所附 node-pty 1.1.0-beta43 的配套 ConPTY 运行时。
- 上游项目：https://github.com/microsoft/terminal
- 随附 LICENSE 保留来源包的 MIT 许可及版权声明。

| 文件 | SHA-256 |
|---|---|
| conpty.dll | 91e5be7f108d47dd94fd3db0d909d7635cc3f836b77a4440e4d0cbe925198b82 |
| OpenConsole.exe | 96a068b94509b966e0a81e89bb87154ea573674d9a52b359dc6b59147dae43bb |

Tauri 将两个运行时文件映射到应用可执行文件旁，开发构建也使用相同的资源映射。
启动时使用绝对路径预加载 conpty.dll 并检查所需导出；portable-pty 随后复用这个模块。
缺失组件时明确报告启动失败，避免静默退回系统控制台而掩盖部署问题。

升级时必须一起替换两个同版本、同架构文件，并更新版本、校验值及对应许可。
当前仅随包提供 x64 组件；其他架构需准备相匹配的运行时。
