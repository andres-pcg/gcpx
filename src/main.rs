//! gcpx CLI entry point.

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use std::io;

use gcpx::commands::{
    ReauthOptions, delete_context, interactive_switch, login_context, reauth,
    reauth::{
        cancel as reauth_cancel, complete as reauth_complete, start as reauth_start,
        worker as reauth_worker,
    },
    run_with_context, save_context, status, switch_context,
    use_cmd::{
        Shell as GcpxShell, clear_default, export_auto, export_unuse, export_use, read_default,
        set_default, show_default,
    },
};
use gcpx::config::{get_current_tracking, list_contexts, load_context_metadata};
use gcpx::init::snippet as init_snippet;
use gcpx::prompt;
use gcpx::workspace::find_workspace_config;
use std::env;

#[derive(Parser)]
#[command(name = "gcpx")]
#[command(author, version, about = "GCP Context Switcher - manage multiple gcloud accounts", long_about = None)]
#[command(
    after_help = "Scripts and AI agents: run `gcpx agents` for a usage guide \
(non-interactive use, JSON output, exit codes, two-step sign-in).\n\n\
Exit codes: 0 ok · 1 error · 2 usage error · 3 re-authentication needed (status) · \
`gcpx run` exits with the wrapped command's code."
)]
struct Cli {
    /// Never prompt: accept the safe default for any confirmation
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Save current gcloud state as a named context
    Save {
        /// Name for the context
        name: String,
        /// Quiet mode - hide sensitive details (account, project, etc.)
        #[arg(short, long)]
        quiet: bool,
        /// Skip saving kubectl context
        #[arg(long)]
        no_kubectl: bool,
        /// Skip kubectl validation (save as-is, for automation)
        #[arg(long)]
        force: bool,
    },
    /// Switch to a saved context
    Switch {
        /// Context name (interactive if omitted)
        name: Option<String>,
        /// Use context from workspace config (nearest .gcpx.toml)
        #[arg(long)]
        workspace: bool,
        /// Quiet mode - hide sensitive details (account, project, etc.)
        #[arg(short, long)]
        quiet: bool,
        /// No output (for shell hooks; errors still to stderr)
        #[arg(long)]
        silent: bool,
    },
    /// Print the currently active context (for shell prompts)
    Current {
        /// JSON output: {"context": ..., "source": "shell"|"global"}
        #[arg(long)]
        json: bool,
    },
    /// List all saved contexts
    List {
        /// JSON output with account, project and gcloud config per context
        #[arg(long)]
        json: bool,
    },
    /// Run a command with a specific context (isolated)
    ///
    /// The command gets the context's GOOGLE_APPLICATION_CREDENTIALS,
    /// CLOUDSDK_ACTIVE_CONFIG_NAME, KUBECONFIG and GCPX_CONTEXT; gcpx exits
    /// with the command's exit code. Best choice for scripts and AI agents.
    #[command(
        after_help = "Examples:\n  gcpx run work -- gcloud projects list --format=json\n  gcpx run work -- kubectl get pods\n  gcpx run work -- terraform plan"
    )]
    Run {
        /// Context name to use
        name: String,
        /// Print a banner to stdout describing the wrapped invocation
        #[arg(short, long)]
        verbose: bool,
        /// Command and arguments to run
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
    /// Delete a saved context
    Delete {
        /// Context name to delete
        name: String,
        /// Also delete the gcloud configuration
        #[arg(long)]
        gcloud_config: bool,
    },
    /// Set up (or re-authenticate) a context: sign in, pick a project, save
    ///
    /// Run without arguments for a guided setup that asks for the context
    /// name and lets you pick a project from your account.
    Login {
        /// Context name (prompted if omitted)
        name: Option<String>,
        /// Default project for the context (prompted with a picker if omitted)
        #[arg(long)]
        project: Option<String>,
        /// Quiet mode - hide sensitive details (account, project, etc.)
        #[arg(short, long)]
        quiet: bool,
    },
    /// Show credential health (gcloud session + ADC) for saved contexts
    ///
    /// Exits with code 3 when a gcloud session needs re-authentication.
    Status {
        /// Context name (all contexts if omitted)
        name: Option<String>,
        /// JSON output (stable schema; see `gcpx agents`)
        #[arg(long)]
        json: bool,
    },
    /// Refresh expired sessions (Google's 16h session control) for contexts
    ///
    /// Only touches what is stale. gcloud reauth uses the terminal password
    /// prompt when your organisation allows it, otherwise the browser (or a
    /// link to open when no browser is available).
    ///
    /// Without a terminal (scripts, AI agents), use the two-step flow:
    /// `--start` prints a sign-in link; the user opens it, signs in and gets a
    /// verification code; `--code <CODE>` completes the sign-in.
    #[command(
        after_help = "Examples:\n  gcpx reauth work\n  gcpx reauth --all --adc\n  gcpx reauth work --start --json      # → {\"url\": ...}\n  gcpx reauth work --code 4/0Ab...     # or: echo CODE | gcpx reauth work --code -"
    )]
    Reauth {
        /// Context name(s)
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        names: Vec<String>,
        /// Two-step sign-in: print a link and wait in the background for --code
        #[arg(long, conflicts_with_all = ["code", "cancel", "all"])]
        start: bool,
        /// Two-step sign-in: the verification code from the browser (`-` reads stdin)
        #[arg(long, value_name = "CODE", conflicts_with_all = ["cancel", "all"])]
        code: Option<String>,
        /// Two-step sign-in: discard a started sign-in
        #[arg(long, conflicts_with = "all")]
        cancel: bool,
        /// JSON output on stdout (progress goes to stderr)
        #[arg(long)]
        json: bool,
        /// Every saved context that needs it
        #[arg(long)]
        all: bool,
        /// Also refresh ADC (Terraform, client libraries) — same browser trip
        #[arg(long)]
        adc: bool,
        /// Re-login even if credentials look valid
        #[arg(long)]
        force: bool,
    },
    /// Print a usage guide for scripts and AI agents
    Agents,
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Print shell integration to source in your rc file.
    ///
    /// Example: eval "$(gcpx init zsh)"
    ///
    /// Installs a `gcpx use <name>` shell function and a chpwd auto-switch
    /// hook. Both export per-shell env vars (CLOUDSDK_ACTIVE_CONFIG_NAME,
    /// GOOGLE_APPLICATION_CREDENTIALS, KUBECONFIG) so each terminal has its
    /// own isolated context — no global filesystem mutation.
    Init {
        /// Shell to emit integration for (zsh, bash, fish)
        shell: String,
    },
    /// Set, show, or clear the global default context (used when no .gcpx.toml).
    Default {
        /// Context name (omit to show current default)
        name: Option<String>,
        /// Clear the default
        #[arg(long, conflicts_with = "name")]
        clear: bool,
    },
    /// Internal: emit shell exports for `use`/`unuse`/`auto`.
    /// Invoked by the shell function installed by `gcpx init`.
    #[command(hide = true, name = "__export")]
    Export {
        /// Mode: use, unuse, or auto
        mode: String,
        /// Context name (for `use` only; optional → falls back to workspace/default)
        name: Option<String>,
        /// Shell flavor (zsh, bash, fish)
        #[arg(long)]
        shell: String,
        /// Override workspace `.gcpx.toml` pin (use only)
        #[arg(long)]
        force: bool,
    },
    /// Internal: background half of `gcpx reauth --start`.
    #[command(hide = true, name = "__reauth-worker")]
    ReauthWorker { name: String },
}

/// Exit code for "a gcloud session needs re-authentication" (`gcpx status`).
const EXIT_REAUTH_NEEDED: i32 = 3;

const AGENTS_GUIDE: &str = include_str!("../AGENTS.md");

fn main() -> Result<()> {
    let cli = Cli::parse();
    prompt::set_assume_yes(cli.yes);

    match cli.command {
        Some(Commands::Save {
            name,
            quiet,
            no_kubectl,
            force,
        }) => save_context(&name, quiet, no_kubectl, force)?,
        Some(Commands::Switch {
            name,
            workspace,
            quiet,
            silent,
        }) => {
            if let Some(n) = name {
                switch_context(&n, quiet, silent)?
            } else if workspace {
                let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                match find_workspace_config(&cwd)? {
                    Some((_dir, wc)) => {
                        let ctx = wc.context.as_deref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "Workspace config has no 'context' set. Add context = \"<name>\" to .gcpx.toml"
                            )
                        })?;
                        // Skip switch when already on this context (avoids extra I/O in silent mode)
                        if silent && get_current_tracking() == ctx {
                            return Ok(());
                        }
                        switch_context(ctx, quiet, silent)?
                    }
                    None => anyhow::bail!(
                        "No workspace config found. Add a .gcpx.toml with 'context = \"<name>\"' in this directory or a parent."
                    ),
                }
            } else {
                prompt::require_interactive(
                    "Choosing a context interactively",
                    "Pass a name: `gcpx switch <name>` (see `gcpx list`).",
                )?;
                let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let default_ctx = find_workspace_config(&cwd)
                    .ok()
                    .and_then(|o| o.and_then(|(_, wc)| wc.context));
                interactive_switch(quiet, default_ctx.as_deref())?
            }
        }
        Some(Commands::List { json }) => {
            let current = env::var("GCPX_CONTEXT")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(get_current_tracking);
            let ctxs = list_contexts()?;
            if json {
                let default = read_default()?;
                let items: Vec<_> = ctxs
                    .iter()
                    .map(|c| {
                        let m = load_context_metadata(c).ok().flatten();
                        serde_json::json!({
                            "name": c,
                            "account": m.as_ref().and_then(|m| m.account.clone()),
                            "project": m.as_ref().and_then(|m| m.project.clone()),
                            "gcloud_config": m.as_ref().map(|m| m.gcloud_config.clone()),
                            "kubectl_context": m.as_ref().and_then(|m| m.kubectl_context.clone()),
                            "current": *c == current,
                            "default": default.as_deref() == Some(c.as_str()),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "contexts": items }))?
                );
            } else if ctxs.is_empty() {
                println!("No contexts found. Create one with 'gcpx login'");
            } else {
                for ctx in ctxs {
                    if ctx == current {
                        println!("* {} (active)", ctx);
                    } else {
                        println!("  {}", ctx);
                    }
                }
            }
        }
        Some(Commands::Current { json }) => {
            // Prefer the per-shell env var set by `gcpx use` / the auto-switch
            // hook. Falls back to the legacy .current file (only updated by
            // `gcpx switch`) so older integrations keep working.
            let (ctx, source) = match env::var("GCPX_CONTEXT") {
                Ok(v) if !v.is_empty() => (Some(v), Some("shell")),
                _ => match get_current_tracking() {
                    t if t == "none" => (None, None),
                    t => (Some(t), Some("global")),
                },
            };
            if json {
                println!("{}", serde_json::json!({"context": ctx, "source": source}));
            } else {
                print!("{}", ctx.as_deref().unwrap_or("none"));
            }
        }
        Some(Commands::Run { name, cmd, verbose }) => {
            let code = run_with_context(&name, &cmd, verbose)?;
            std::process::exit(code);
        }
        Some(Commands::Delete {
            name,
            gcloud_config,
        }) => {
            delete_context(&name, gcloud_config)?;
        }
        Some(Commands::Login {
            name,
            project,
            quiet,
        }) => {
            login_context(name.as_deref(), project.as_deref(), quiet)?;
        }
        Some(Commands::Status { name, json }) => {
            if status(name.as_deref(), json)? {
                std::process::exit(EXIT_REAUTH_NEEDED);
            }
        }
        Some(Commands::Reauth {
            names,
            all,
            adc,
            force,
            start,
            code,
            cancel,
            json,
        }) => {
            let opts = ReauthOptions { adc, force, json };
            if start || code.is_some() || cancel {
                let [name] = names.as_slice() else {
                    anyhow::bail!("--start, --code and --cancel take exactly one context name.");
                };
                if start {
                    reauth_start(name, opts)?;
                } else if let Some(code) = code {
                    reauth_complete(name, &code, opts)?;
                } else {
                    reauth_cancel(name, opts)?;
                }
            } else {
                reauth(&names, all, opts)?;
            }
        }
        Some(Commands::ReauthWorker { name }) => reauth_worker(&name)?,
        Some(Commands::Agents) => print!("{}", AGENTS_GUIDE),
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, name, &mut io::stdout());
        }
        Some(Commands::Init { shell }) => match init_snippet(&shell) {
            Some(s) => print!("{}", s),
            None => anyhow::bail!("Unsupported shell '{}'. Use zsh, bash, or fish.", shell),
        },
        Some(Commands::Default { name, clear }) => {
            if clear {
                clear_default()?;
            } else if let Some(n) = name {
                set_default(&n)?;
            } else {
                match read_default()? {
                    Some(_) => show_default()?,
                    None => println!("(no default set)"),
                }
            }
        }
        Some(Commands::Export {
            mode,
            name,
            shell,
            force,
        }) => {
            let sh = GcpxShell::parse(&shell)?;
            let out = match mode.as_str() {
                "use" => export_use(name.as_deref(), force, sh)?,
                "unuse" => export_unuse(sh)?,
                "auto" => export_auto(sh)?,
                other => anyhow::bail!("Unknown __export mode '{}'.", other),
            };
            print!("{}", out);
        }
        None => {
            if !prompt::is_interactive() {
                // No terminal (script / agent): show help instead of a picker.
                Cli::command().print_help()?;
                std::process::exit(2);
            }
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let default_ctx = find_workspace_config(&cwd)
                .ok()
                .and_then(|o| o.and_then(|(_, wc)| wc.context));
            interactive_switch(false, default_ctx.as_deref())?
        }
    }

    Ok(())
}
