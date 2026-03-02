# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Workspace/project config: `.gcpx.toml` or `.gcpx.json` in the project root (or any parent up to `$HOME`) can set `context = "<name>"` to pin that directory to a gcpx context
- `gcpx switch --workspace` switches to the context defined in the nearest workspace config
- `gcpx switch --workspace --silent` for use in shell hooks (no output unless error)
- Interactive switch (`gcpx` or `gcpx switch` with no name) pre-selects the workspace context in the menu when present
- Auto-switch: add a shell hook (e.g. zsh `chpwd`/`precmd`, bash `PROMPT_COMMAND`) that runs `gcpx switch --workspace --silent` so the workspace context is applied automatically when you `cd` or open a terminal

## [0.1.0] - 2026-02-03

### Added

- Initial release
- `save` command - Save current gcloud + ADC credentials as a named context
- `switch` command - Switch between saved contexts (config + ADC)
- `list` command - View all saved contexts with active indicator
- `current` command - Print active context name (for shell prompts)
- `run` command - Execute commands with a specific context (isolated)
- `login` command - Re-authenticate and auto-save credentials
- `delete` command - Remove saved contexts
- `completions` command - Generate shell completions (bash, zsh, fish, powershell)
- Interactive mode - Select context from menu when no command specified
- Secure credential storage (0600 permissions on Unix)
- Cross-platform support (Linux, macOS, Windows)
