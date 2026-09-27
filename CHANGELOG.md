# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-09-27

### Added — scripts, CI and AI agents

- **Two-step sign-in:** `gcpx reauth <context> --start` prints a Google sign-in link and leaves the sign-in waiting in the background; `gcpx reauth <context> --code <CODE>` (or `--code -` to read stdin) completes it with the verification code the browser shows; `--cancel` discards it. Works for agents and scripts that can't type into a running process. The started sign-in expires after 10 minutes. macOS and Linux.
- **`--json` output** for `list`, `current`, `status` and `reauth`, with a documented, stable schema.
- **Exit codes:** `gcpx status` exits with `3` when a gcloud session needs re-authentication (`1` stays for errors).
- **Global `--yes`:** accept the safe default for any confirmation.
- **`gcpx agents`** prints a usage guide for agents (also in `AGENTS.md`), and `skills/gcpx/SKILL.md` is an installable agent skill.
- **Guided `gcpx login`:** run without arguments to set up a context step by step. It asks for the context name, signs in once (gcloud + ADC), lists the account's projects in a searchable picker (or lets you type one / skip), saves, and offers to make it the default context. `gcpx login <name> --project <id>` does the same non-interactively.

### Changed

- **`gcpx run` exits with the wrapped command's exit code** (was always `1` on failure, plus an extra error line), and sets `KUBECONFIG` and `GCPX_CONTEXT` like `gcpx use` does.
- **Nothing blocks or fails with "not a terminal" any more.** Without a terminal, `gcpx` alone prints help, prompts are replaced by their safe default under `--yes` or by an error naming the flag to use, and `gcpx reauth` forces the browser sign-in (no password prompt is possible).
- **`gcpx save`** no longer saves a GKE kubectl context it can't verify (no gcloud project set) without asking; without a terminal it saves without kubectl and says so.
- `gcpx login` says gcloud will open the browser *or print a link*.
- README: new **Getting Started** guide and a **Scripts, CI and AI agents** section.

### Fixed

- `gcpx status` suggested only the gcloud fix when both the gcloud session and ADC of a context were stale.

## [0.4.0] - 2026-09-25

### Added — expired sessions (Google Cloud session control)

- **`gcpx status [name]`** — checks each context's gcloud session and stored ADC against Google and reports `ok` / `reauth required` / `revoked`, plus account mismatches (gcloud config or ADC pointing at a different account than the context was saved for). Exits non-zero when a gcloud session needs reauth.
- **`gcpx reauth <names…> | --all [--adc] [--force]`** — refreshes only what is stale. gcloud reauth runs inside the context's configuration (password prompt in the terminal when the org allows it, browser otherwise). `--adc` also refreshes ADC in the same browser trip (`--update-adc`), stores it in the context and restores the global ADC. One sign-in per account per run.
- **Safe ADC adoption** — a working ADC already at gcloud's well-known path is adopted into a context without a browser, only when Google confirms it's the context's saved account.
- Explicit `gcpx use` / `gcpx switch` warn on stderr when the context's session has expired (`GCPX_NO_AUTH_CHECK=1` disables).

### Changed

- **`gcpx login`** uses a single browser flow (`gcloud auth login --update-adc`) instead of two, and no longer saves the context when the login fails.

## [0.3.0] - 2026-05-27

### Added — per-shell context isolation (major)

- **`gcpx init <shell>`** — emits zsh/bash/fish integration. Drop `eval "$(gcpx init zsh)"` in `.zshrc` and you get a `gcpx use` shell function plus an auto-switch `chpwd` hook.
- **`gcpx use <name>`** — pins the current shell to a context by exporting `GOOGLE_APPLICATION_CREDENTIALS`, `CLOUDSDK_ACTIVE_CONFIG_NAME`, `KUBECONFIG`, and `GCPX_CONTEXT`. No global filesystem mutation, so switching in one terminal cannot affect another. Replaces `gcpx switch` for daily use.
- **`gcpx unuse`** — drops the shell pin and re-applies workspace/default resolution.
- **`gcpx default [name]`** — set / show / clear (`--clear`) the global fallback context used in shells with no workspace pin.
- **Auto-switch hook** runs on `chpwd` and prints `gcpx: auto-switched to '<name>'` to stderr on every implicit change — credential swaps are never silent.
- **Workspace pin wins over shell pin.** `.gcpx.toml` is a project-level safety guard; `gcpx use` inside a pinned project is refused unless you pass `--force` (which sets a sticky `GCPX_SHELL_PIN_FORCE` that survives `cd`s).
- **Per-context kube snapshot.** `save` now also stores a minified/flattened kubeconfig at `~/.config/gcpx/<name>/kube.config` so `gcpx use` can point `KUBECONFIG` at it for full per-shell kubectl isolation.
- **`gcpx current`** now prefers `$GCPX_CONTEXT` (the per-shell value) and only falls back to the legacy `.current` file — so prompt integrations reflect the actual shell context.

### Added — earlier in unreleased window

- `save --no-kubectl` flag to skip saving kubectl context
- `save --force` flag to skip kubectl validation (for automation/scripts)
- Kubectl context validation: warns when kubectl context belongs to a different GCP project than gcloud
- Workspace/project config: `.gcpx.toml` or `.gcpx.json` in the project root (or any parent up to `$HOME`) can set `context = "<name>"`
- `gcpx switch --workspace` and `--silent` (kept for backwards compatibility; superseded by `gcpx init` + `gcpx use`)

### Changed

- **`gcpx run` no longer prints a banner by default.** The `Running with context...` line broke pipes (e.g. `gcpx run x gcloud ... --format=json | jq`). It is now silent by default; pass `-v`/`--verbose` to print the banner to **stdout** (intentionally not stderr — GCP Cloud Logging and similar log aggregators map stderr to ERROR severity by default, and the banner is informational, not an error).

### Security

Hardening pass on the new code (independent review):

- `GCPX_HOME` and `GCPX_GCLOUD_DIR` are now ignored unless `GCPX_ALLOW_TEST_ENV=1` is also set. Prevents an attacker who can poison a victim's environment from redirecting credential storage to an attacker-controlled directory.
- `~/.config/gcpx/.default` content is validated through `validate_context_name` before use (defense-in-depth against path-traversal via tampered file).
- ADC and kube snapshot files are now created atomically with mode `0600` via `O_CREAT | O_EXCL` (closes a TOCTOU window where the secret briefly existed at the process umask).
- Storage dir `~/.config/gcpx/` and per-context dirs are now `0700`.
- Auto-switch hook **refuses `.gcpx.toml` in untrusted (world-writable, non-sticky) directories**. Bypass with `GCPX_TRUST_ALL_WORKSPACES=1`. Combined with the stderr notice, silent credential redirection via a hostile `cd` target is mitigated.
- Untrusted strings (kubectl context names) are sanitized of ASCII control chars before being printed, preventing terminal-escape spoofing.

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
