//! gcpx CLI entry point.

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use std::io;

use gcpx::commands::{
    delete_context, interactive_switch, login_context, run_with_context, save_context,
    switch_context,
    use_cmd::{
        Shell as GcpxShell, clear_default, export_auto, export_unuse, export_use, read_default,
        set_default, show_default,
    },
};
use gcpx::config::{get_current_tracking, list_contexts};
use gcpx::init::snippet as init_snippet;
use gcpx::workspace::find_workspace_config;
use std::env;

#[derive(Parser)]
#[command(name = "gcpx")]
#[command(author, version, about = "GCP Context Switcher - manage multiple gcloud accounts", long_about = None)]
struct Cli {
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
    Current,
    /// List all saved contexts
    List,
    /// Run a command with a specific context (isolated)
    Run {
        /// Context name to use
        name: String,
        /// Print a banner to stderr describing the wrapped invocation
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
    /// Re-authenticate and save credentials for a context
    Login {
        /// Context name to authenticate
        name: String,
        /// Quiet mode - hide sensitive details (account, project, etc.)
        #[arg(short, long)]
        quiet: bool,
    },
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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
                let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let default_ctx = find_workspace_config(&cwd)
                    .ok()
                    .and_then(|o| o.and_then(|(_, wc)| wc.context));
                interactive_switch(quiet, default_ctx.as_deref())?
            }
        }
        Some(Commands::List) => {
            let current = get_current_tracking();
            let ctxs = list_contexts()?;
            if ctxs.is_empty() {
                println!("No contexts found. Create one with 'gcpx save <name>'");
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
        Some(Commands::Current) => {
            // Prefer the per-shell env var set by `gcpx use` / the auto-switch
            // hook. Falls back to the legacy .current file (only updated by
            // `gcpx switch`) so older integrations keep working.
            match env::var("GCPX_CONTEXT") {
                Ok(v) if !v.is_empty() => print!("{}", v),
                _ => print!("{}", get_current_tracking()),
            }
        }
        Some(Commands::Run { name, cmd, verbose }) => {
            run_with_context(&name, &cmd, verbose)?;
        }
        Some(Commands::Delete {
            name,
            gcloud_config,
        }) => {
            delete_context(&name, gcloud_config)?;
        }
        Some(Commands::Login { name, quiet }) => {
            login_context(&name, quiet)?;
        }
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
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let default_ctx = find_workspace_config(&cwd)
                .ok()
                .and_then(|o| o.and_then(|(_, wc)| wc.context));
            interactive_switch(false, default_ctx.as_deref())?
        }
    }

    Ok(())
}
