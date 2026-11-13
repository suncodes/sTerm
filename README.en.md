# sTerm

[中文文档](README.md)

sTerm is a lightweight Windows terminal optimized for everyday AI coding-agent workflows. It combines Tauri 2, Rust ConPTY, and xterm.js without bundling a browser runtime.

## Highlights

- PowerShell 7, Windows PowerShell, and cmd sessions powered by ConPTY
- Tabs, saved workspaces, folders, and recent agent-session discovery
- Optional status indicators for Claude Code, Codex, and Pi
- Search, keyboard shortcuts, configurable fonts, and an optional always-on-top status overlay

## Requirements

- Windows 10 1809 (Build 17763) or later, x64
- WebView2 Runtime
- To build from source: Node.js 20+, Python 3.8+, and Rust stable with the MSVC toolchain

## Build

~~~powershell
npm ci
npm test
cargo test --manifest-path .\src-tauri\Cargo.toml
npm run tauri build
~~~

Use .\scripts\package.ps1 to generate the portable ZIP after building.

## Releasing a version

package.json is the only version source that needs manual input. Do not edit Cargo, Tauri, or lock-file versions individually; the following command synchronizes derived metadata and then validates it:

~~~powershell
npm run version:set -- 0.1.4
npm run version:check
~~~

After completing the code and CHANGELOG.md, commit them so the working tree is clean. One command then synchronizes the version, runs checks, creates the release commit and tag, and atomically pushes `main` and the tag:

~~~powershell
npm run release -- 0.1.4
~~~

The script asks you to type `v0.1.4` before it commits or pushes; use `-Approve` only for an explicitly approved non-interactive run. To intentionally include all current uncommitted changes, add `-IncludeWorkingTree`:

~~~powershell
npm run release -- 0.1.4 -IncludeWorkingTree -Approve
~~~

Pushing the tag makes GitHub Actions validate the tag and metadata, then build the installer and portable archive. The script does not upload binaries directly.

## Privacy and integrations

sTerm has no telemetry or analytics endpoint by default and does not write terminal input or output to its diagnostic log. It does not modify PowerShell profiles or agent configuration automatically. Optional integrations are installed only from Settings → Integrations.

See [PRIVACY.md](PRIVACY.md), [integrations/README.md](integrations/README.md), and [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for details.

## License

sTerm is licensed under the [Apache License 2.0](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for distributed third-party components.
