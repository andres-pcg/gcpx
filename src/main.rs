//! gcpx CLI entry point.

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use std::io;

use gcpx::commands::{
    delete_context, interactive_switch, login_context, run_with_context, save_context,
    switch_context,
};
use gcpx::config::{get_current_tracking, list_contexts};

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
    },
    /// Switch to a saved context
    Switch {
        /// Context name (interactive if omitted)
        name: Option<String>,
        /// Quiet mode - hide sensitive details (account, project, etc.)
        #[arg(short, long)]
        quiet: bool,
    },
    /// Print the currently active context (for shell prompts)
    Current,
    /// List all saved contexts
    List,
    /// Run a command with a specific context (isolated)
    Run {
        /// Context name to use
        name: String,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Save { name, quiet }) => save_context(&name, quiet)?,
        Some(Commands::Switch { name, quiet }) => {
            if let Some(n) = name {
                switch_context(&n, quiet)?
            } else {
                interactive_switch(quiet)?
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
            print!("{}", get_current_tracking());
        }
        Some(Commands::Run { name, cmd }) => {
            run_with_context(&name, &cmd)?;
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
        None => interactive_switch(false)?,
    }

    Ok(())
}
