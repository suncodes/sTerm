# Changelog

All notable changes to sTerm are documented in this file.

## [Unreleased]

## [0.2.8] - 2026-11-28

### Added

- Automatically connect Claude Code and Codex launched in sTerm shells to a structured remote Agent page, with shared native sessions, history, streaming output, model settings, execution permissions, and turn controls.
- Support native approvals, plan confirmation, questions, MCP forms and authorization links, Codex input queues, visible retry states, subagent activity, and confirmed background task output and duration.

### Changed

- Present approvals and questions above the composer with readable prompts, compact paginated choices, preserved drafts, fixed actions, and responsive desktop columns; expand pending asynchronous questions by default while respecting manual collapse.
- Use lightweight thinking and command rows, preserve interleaved streaming order, reduce repeated Agent labels, and keep completed task history and runtime notices out of the active task area.
- Share terminal and Agent views of the same tab under the same permission settings, retain terminal takeover and release, and use matching Terminal / Agent navigation on both pages.

### Fixed

- Keep unanswered asynchronous questions available while Codex continues working, prevent duplicate shared-session counts, and restore shell state after Agent exit.
- Handle native safety blocks separately from approvals, expose available continuation details and explicit confirmation on mobile, and reject stale or duplicate recovery requests.

## [0.2.7] - 2026-11-24

### Fixed

- Allow Feishu follow-up whenever the bound Agent is ready, regardless of earlier terminal editing; keep completion notifications independent and reject only current busy states, remote control, or concurrent input during submission.
- Use the session name as the Feishu completion card title when name display is enabled, and display completion times in UTC+08:00.

## [0.2.6] - 2026-11-24

### Added

- Add optional Feishu completion notifications for terminal sessions, topic-bound plain-text follow-up, encrypted credentials, and links back to shared remote sessions.
- Add explicit completion events across native Agent protocols and Pi agent_settled integration.

### Fixed

- Persist the Feishu enable switch when configuration is incomplete or the settings page changes; support optional user lists and explicit access for all group members.
- Keep terminal focus reports and device-query replies from invalidating follow-up input, while preserving remote control and concurrent input protection.
- Use one-time pairing links when no remote access code is configured, prioritize existing browser authorization or access-code login, and allow authorized users to refresh links from the original topic.
- Submit complete multiline Codex prompts on Windows without sending only the first paragraph or leaving the remaining text in the input box.

## [0.2.5] - 2026-11-19

### Added

- Add remote Shift and Alt modifier buttons alongside Ctrl, with stacked navigation combinations, one-shot reset, and preserved modifiers during held-key repetition.

## [0.2.4] - 2026-11-18

### Added

- Add remote Backspace and Delete keys, move Enter to the main toolbar, and support holding deletion and arrow keys with cancellation on release, control changes, and disconnect; keep all keys visible in a compact two-row mobile layout.

## [0.2.3] - 2026-11-18

### Added

- Add optional six-digit HTTPS remote access code login alongside QR pairing, with single-entry setup, desktop show/hide/copy controls, Windows-protected recoverable storage and salted verification, 30-day browser authorization, revocation, and rate limits; preserve login with legacy hash-only records until reset.

## [0.2.2] - 2026-11-17

### Added

- Add optional QR-paired remote viewing and single-controller input for explicitly shared terminal sessions.
- Support Cloudflare Tunnel and ngrok with on-demand agents, Windows-protected credentials, and optional Cloudflare tunnel/DNS setup.
- Add an isolated remote development configuration and focused remote gateway, terminal-mirror, and credential tests.
- Move remote access into Settings with connection, permissions, and public-tunnel sections; add saved sharing defaults, per-session exceptions, and device revocation.
- Match the remote browser UI to the desktop theme, with a mobile session drawer, wide-screen tabs, per-session drafts, shortcut panels, and a fixed rectangular text send button.
- Add browser-local remote input history with search, deduplication, deletion, and draft recall with undo; keep the latest 100 entries without automatically executing recalled input.

### Fixed

- Skip optional background keepalive when navigator or Web Locks is unavailable, so remote controller tests also run on Node.js 20.
- Prevent orphaned tunnel agents with Windows process jobs, scoped cleanup, confirmed stop, and per-directory locks; reuse repeated connections and enable ngrok dashboard Stop.

## [0.2.1] - 2026-11-13

### Changed

- Use the application icon for the NSIS installer and uninstaller.

### Fixed

- Keep an edge-docked empty Agent overlay compact as a neutral handle that expands on click.
- Restore the workspace “Move to folder…” target chooser as an edge-aware cascading folder menu, and report failed moves.

## [0.2.0] - 2026-11-13

### Changed

- Use package.json as the single version source and synchronize derived release metadata with a checked script.
- Add a guarded one-command release script for versioning, validation, Git commit/tag, and atomic push.
- Add four-edge docking for the Agent status overlay, with compact status-dot strips and persistent placement.
- Keep the enabled Agent overlay visible with an explicit empty state when no Agent session is active.
- Redesign integration settings with installation status for PowerShell and Pi, while identifying Claude Code and Codex as native integrations.

## [0.1.3] - 2026-11-13

### Fixed

- Collect release artifacts as a PowerShell array before generating SHA-256 checksums.

## [0.1.1] - 2026-11-12

### Fixed

- Build the frontend before Rust tests in the tag-triggered release workflow.

## [0.1.0] - 2026-11-12

### Added

- Initial public preview release.
- Windows x64 NSIS and portable ZIP release workflow.
- Apache-2.0 licensing, privacy documentation, security policy, and contribution guide.
