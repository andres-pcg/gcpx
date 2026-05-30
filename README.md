# gcpx - GCP Context Switcher

[![CI](https://github.com/andres-pcg/gcpx/actions/workflows/ci.yml/badge.svg)](https://github.com/andres-pcg/gcpx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A fast, secure CLI tool for managing multiple Google Cloud Platform accounts with seamless switching of both gcloud configurations and ADC credentials.

## The Problem

Managing multiple GCP accounts is painful:

- You have to re-authenticate every time you switch accounts
- Application Default Credentials (ADC) are stored in a single file that gets overwritten
- Constantly running `gcloud auth application-default login` is tedious
- Easy to accidentally run commands against the wrong project

## The Solution

`gcpx` saves separate ADC credentials for each account and swaps them automatically when switching. No more re-authentication!

## Features

| Feature | Description |
|---------|-------------|
| **Save** | Store current gcloud + ADC + kubectl context as a named context |
| **Switch** | Instantly switch between contexts (gcloud + ADC + kubectl) |
| **List** | View all saved contexts with active indicator |
| **Run** | Execute commands with a specific context (isolated) |
| **Login** | Re-authenticate and auto-save credentials |
| **Delete** | Remove saved contexts |
| **Completions** | Shell completion for bash, zsh, fish, powershell |
| **kubectl** | Automatically saves and restores kubectl context (with project validation) |
| **Smart skip** | Skips switching if already on the requested context |
| **Workspace config** | Pin a project to a context via `.gcpx.toml` (discovered from cwd and parents) |
| **`use` (per-shell)** | `gcpx use <name>` activates a context in *this terminal only* — env-var based, no global mutation, so terminals can't collide |
| **Shell integration** | `eval "$(gcpx init zsh\|bash\|fish)"` installs a `use` function and a `chpwd` auto-switch hook |
| **Global default** | `gcpx default <name>` sets a fallback context for shells with no `.gcpx.toml` |

## Installation

### From Source (Cargo)

```bash
cargo install gcpx
```

### Homebrew (macOS/Linux)

```bash
brew tap andres-pcg/tap
brew install gcpx
```

### From Releases

Download the latest binary from the [Releases](https://github.com/andres-pcg/gcpx/releases) page.

## Quick Start

### Initial Setup

```bash
# Authenticate with your first account
gcloud auth login
gcloud auth application-default login

# Save it as a context
gcpx save work

# Authenticate with another account
gcloud config configurations create personal
gcloud auth login
gcloud auth application-default login

# Save it too
gcpx save personal
```

### Daily Usage

```bash
# Switch between accounts instantly (no re-auth!)
gcpx switch work
gcpx switch personal

# Or use interactive mode
gcpx

# List all contexts
gcpx list
# Output:
# * work (active)
#   personal

# Check current context
gcpx current
# Output: work
```

### Per-shell context isolation (recommended)

`gcpx switch` mutates `~/.config/gcloud/` globally, so switching in one terminal silently changes the context for every other open terminal. To avoid that, gcpx ships a shell integration that uses per-process **env vars** instead — each terminal is sealed off from every other.

**One-time setup.** Add to your shell rc:

```bash
# ~/.zshrc
eval "$(gcpx init zsh)"

# ~/.bashrc
eval "$(gcpx init bash)"

# ~/.config/fish/config.fish
gcpx init fish | source
```

That installs:
- a `gcpx use <name>` shell function that exports `GOOGLE_APPLICATION_CREDENTIALS`, `CLOUDSDK_ACTIVE_CONFIG_NAME`, `KUBECONFIG`, and `GCPX_CONTEXT` for the current shell only;
- a `chpwd` hook that auto-applies the context when you `cd` into a project with a `.gcpx.toml`.

**Daily use:**

```bash
gcpx use work          # pin THIS shell to 'work'
gcpx unuse             # drop the pin, re-resolve workspace/default
gcpx default work      # fallback for shells with no workspace pin and no explicit use
echo $GCPX_CONTEXT     # what is this shell on right now?
```

`gcpx switch` still exists and still mutates global state — kept for backwards compatibility, but `gcpx use` is the recommended path.

### Workspace / project-level config

Create a `.gcpx.toml` (or `.gcpx.json`) in the project root:

```toml
# .gcpx.toml
context = "work"
```

gcpx walks up from your **current working directory** to your home dir looking for this file. From any subdirectory of the project, the same config applies.

With the shell integration installed (above), the auto-switch hook runs on every `cd` and applies the workspace context automatically. You will see `gcpx: auto-switched to '<name>'` on stderr each time the context actually changes — credential swaps are never silent.

**Resolution priority** (auto-switch hook):

1. `gcpx use <name> --force` — sticky shell pin survives `cd`s
2. Trusted workspace `.gcpx.toml` — wins over a non-force shell pin (project-level safety guard beats session pin)
3. `gcpx use <name>` (non-force) — used outside any workspace
4. `gcpx default <name>` — fallback when nothing else applies

`gcpx use <name>` inside a directory pinned to something else is **refused** with a clear error. Re-run with `--force` to override for the lifetime of that shell only.

**Trust check:** the auto-switch hook refuses `.gcpx.toml` in world-writable directories without the sticky bit (e.g. `/tmp/...`). Override with `GCPX_TRUST_ALL_WORKSPACES=1` if you know what you're doing.

**Important:** The config we use is the one in the **current working directory** (or the nearest parent that has a `.gcpx.toml`). So if you open your IDE with a **parent** folder (e.g. `MINED`) as the workspace root, the terminal starts in `MINED` and gcpx will use `MINED/.gcpx.toml` if it exists — not `athenea/.gcpx.toml`. To use the config in a subfolder, either open that folder as the workspace root, or `cd` into it (the hook will then run and use that folder's config).

#### Legacy hook (still works, but superseded)

If you previously installed a `chpwd`/`precmd` hook that calls `gcpx switch --workspace --silent`, you can keep it — it still works. The new `eval "$(gcpx init zsh)"` does everything that did, plus per-shell isolation. Switching is recommended.

### Privacy Mode (Quiet Flag)

When streaming or sharing your screen, use the `-q` or `--quiet` flag to hide sensitive details like account email, project ID, and kubectl context:

```bash
# Normal output shows details
gcpx switch work
# Output:
# Switched to 'work' successfully!
#   account: you@company.com
#   project: my-secret-project
#   kubectl: gke_my-cluster

# Quiet mode hides sensitive info
gcpx switch work -q
# Output:
# Switched to 'work' successfully!

# Also works with save and login
gcpx save my-context --quiet
gcpx login work -q
```

### Run Commands with Specific Context

Run a command with a different context without switching globally:

```bash
# Run gcloud command with 'personal' context
gcpx run personal gcloud compute instances list

# Run terraform with specific context
gcpx run work terraform apply

# Pipe-safe by default — no stdout banner
gcpx run work gcloud compute instances list --format=json | jq '.[].name'

# Add -v to print the "Running with context..." banner (to stderr)
gcpx run -v work gcloud compute instances list
```

### Re-authenticate a Context

```bash
# Opens browser for auth, then auto-saves
gcpx login work
```

### Delete a Context

```bash
# Delete just the saved credentials
gcpx delete old-project

# Also delete the gcloud configuration
gcpx delete old-project --gcloud-config
```

## Shell Prompt Integration

Show the active GCP context in your shell prompt to always know which account you're using.

### Powerlevel10k

If you use [Powerlevel10k](https://github.com/romkatv/powerlevel10k), add a custom segment:

1. Edit `~/.p10k.zsh` and add `gcpx` to your prompt elements (inside the anonymous function, around line 40-80):

```bash
typeset -g POWERLEVEL9K_RIGHT_PROMPT_ELEMENTS=(
    # ... other elements ...
    gcpx                    # gcpx context
    gcloud                  # google cloud cli
    # ... other elements ...
)
```

2. Add this function at the **END of the file**, but **BEFORE** the options restoration lines. Look for the closing `}` of the anonymous function and add the function AFTER it:

```bash
}  # <-- This closes the anonymous function (around line 1700+)

################################[ gcpx: GCP context from gcpx ]################################
# Custom segment for gcpx - MUST be placed OUTSIDE the anonymous function
function prompt_gcpx() {
  # Reads the per-shell env var set by `gcpx use` / the auto-switch hook —
  # faster than shelling out to `gcpx current` on every prompt, and shows
  # this shell's actual context (not a global).
  local ctx="${GCPX_CONTEXT:-}"
  if [[ -n "$ctx" ]]; then
    p10k segment -f 33 -i '☁' -t "$ctx"
  fi
}

# Restore original shell options (REQUIRED - do not remove these lines!)
(( ${#p10k_config_opts} )) && setopt ${p10k_config_opts[@]}
'builtin' 'unset' 'p10k_config_opts'
```

> **Warning**: The last two lines that restore shell options are CRITICAL. If they are missing, aliases and other shell features will stop working.

3. Reload: `source ~/.p10k.zsh`

### Oh-My-Zsh (Standard Themes)

Add to your `~/.zshrc` after Oh-My-Zsh is loaded:

```bash
# GCP context in prompt
gcpx_prompt_info() {
    local ctx="${GCPX_CONTEXT:-}"
    if [[ -n "$ctx" ]]; then
        echo "%{$fg[cyan]%}☁ $ctx%{$reset_color%} "
    fi
}

# Prepend to your existing prompt
PROMPT='$(gcpx_prompt_info)'$PROMPT
```

### Starship

If you use [Starship](https://starship.rs/), add to `~/.config/starship.toml`:

```toml
[custom.gcpx]
command = "gcpx current 2>/dev/null"
when = "gcpx current 2>/dev/null"
format = "[$output]($style) "
style = "bold cyan"
symbol = "☁ "
```

### Bash

Add to your `~/.bashrc`:

```bash
gcpx_prompt() {
    local ctx=$(gcpx current 2>/dev/null)
    if [ -n "$ctx" ] && [ "$ctx" != "none" ]; then
        echo "($ctx) "
    fi
}

PS1='$(gcpx_prompt)\u@\h:\w\$ '
```

### Fish

Add to `~/.config/fish/config.fish`:

```fish
function fish_prompt
    set -l ctx (gcpx current 2>/dev/null)
    if test -n "$ctx" -a "$ctx" != "none"
        set_color cyan
        echo -n "☁ $ctx "
        set_color normal
    end
    # ... rest of your prompt
end
```

## Shell Completions

Generate completions for your shell:

```bash
# Bash
gcpx completions bash > /etc/bash_completion.d/gcpx

# Zsh
gcpx completions zsh > "${fpath[1]}/_gcpx"

# Fish
gcpx completions fish > ~/.config/fish/completions/gcpx.fish

# PowerShell
gcpx completions powershell > gcpx.ps1
```

## How It Works

`gcpx` stores credentials and metadata in `~/.config/gcpx/`:

```
~/.config/gcpx/
  .current              # Tracks active context name
  work/
    adc.json            # Saved ADC credentials (0600 permissions)
    metadata.json       # gcloud config, account, project, kubectl context
  personal/
    adc.json
    metadata.json
```

When you `gcpx save <name>`:
1. Captures your current gcloud config name, account, and project
2. Captures your current kubectl context (if kubectl is installed) and validates it matches the gcloud project
3. Copies ADC credentials to the context directory
4. Saves metadata so switching works even if context name differs from gcloud config

**Kubectl validation**: When saving, gcpx checks if your kubectl context belongs to the same GCP project. For GKE clusters (contexts like `gke_<project>_<region>_<cluster>`), it extracts the project and compares it with your active gcloud project. If they don't match, you'll be prompted to choose:
- Save without kubectl context
- Save with the mismatched kubectl context anyway
- Cancel

Use `--no-kubectl` to skip saving kubectl context entirely, or `--force` to skip validation (useful in scripts).

When you `gcpx switch <name>`:
1. Checks if already on the requested context (skips if so - saves time!)
2. Reads metadata to get the correct gcloud configuration name
3. Activates that gcloud configuration
4. Restores the saved ADC credentials
5. Switches kubectl context (if one was saved)
6. Updates the `.current` tracking file

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| macOS (Apple Silicon) | **Tested** | Primary development platform |
| macOS (Intel) | Built | Smoke tested in CI |
| Linux (x86_64) | Built | Smoke tested in CI |
| Linux (ARM64) | Built | Cross-compiled, not runtime tested |
| Linux (musl) | Built | Cross-compiled, not runtime tested |
| Windows (x86_64) | Built | Smoke tested in CI |

Binaries are provided for all platforms above. If you encounter issues on any platform, please [open an issue](https://github.com/andres-pcg/gcpx/issues).

## Security

- Credential files are stored with `0600` permissions (owner read/write only)
- No credentials are transmitted over the network
- Credentials stay in your home directory

## Development

```bash
# Clone the repo
git clone https://github.com/andres-pcg/gcpx.git
cd gcpx

# Build
cargo build

# Run tests
cargo test -- --test-threads=1

# Run clippy
cargo clippy

# Format code
cargo fmt
```

## License

MIT License - see [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
