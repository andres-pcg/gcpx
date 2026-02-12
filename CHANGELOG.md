# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
