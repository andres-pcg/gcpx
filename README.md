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
| **kubectl** | Automatically saves and restores kubectl context |
| **Smart skip** | Skips switching if already on the requested context |

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
  local ctx=$(~/.cargo/bin/gcpx current 2>/dev/null)
  if [[ -n "$ctx" && "$ctx" != "none" ]]; then
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
    local ctx=$(gcpx current 2>/dev/null)
    if [[ -n "$ctx" && "$ctx" != "none" ]]; then
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
2. Captures your current kubectl context (if kubectl is installed)
3. Copies ADC credentials to the context directory
4. Saves metadata so switching works even if context name differs from gcloud config

When you `gcpx switch <name>`:
1. Checks if already on the requested context (skips if so - saves time!)
2. Reads metadata to get the correct gcloud configuration name
3. Activates that gcloud configuration
4. Restores the saved ADC credentials
5. Switches kubectl context (if one was saved)
6. Updates the `.current` tracking file

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
