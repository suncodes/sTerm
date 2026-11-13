# Changelog

All notable changes to sTerm are documented in this file.

## [Unreleased]

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
