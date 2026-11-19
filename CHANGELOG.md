# Changelog

All notable changes to sTerm are documented in this file.

## [Unreleased]

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
