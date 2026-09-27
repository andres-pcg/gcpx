//! Run command implementation - execute commands with a specific context.

use anyhow::{Context, Result, bail};
use std::process::{Command, ExitStatus};

use crate::commands::use_cmd::context_env;
use crate::config::validate_context_name;

/// Runs a command with a specific context without switching globally, and
/// returns the command's exit code so the caller can exit with it unchanged.
///
/// The child gets exactly the environment `gcpx use` would export
/// (`GOOGLE_APPLICATION_CREDENTIALS`, `CLOUDSDK_ACTIVE_CONFIG_NAME`,
/// `KUBECONFIG`, `GCPX_CONTEXT`). The current shell is not affected. This is
/// the recommended way for scripts and AI agents to use a context, since it
/// needs no shell integration and no state between commands.
///
/// `verbose` prints a one-line banner to **stdout** describing the wrapped
/// invocation. Off by default so that piping the wrapped command's stdout
/// (e.g. `gcpx run x gcloud ... --format=json | jq`) stays clean.
///
/// The banner is intentionally on stdout (not stderr) so that when this
/// command is used inside a CI job that ships output to a log aggregator
/// (e.g. GCP Cloud Logging, which maps stderr → ERROR severity by default),
/// the informational banner doesn't get flagged as an error. Users who opt
/// into `-v` while piping accept that the banner will appear in the pipe,
/// just like `curl -v`.
pub fn run_with_context(context_name: &str, cmd: &[String], verbose: bool) -> Result<i32> {
    validate_context_name(context_name)?;
    if cmd.is_empty() {
        bail!("No command specified. Usage: gcpx run <context> -- <command>");
    }

    let env = context_env(context_name)?;
    let program = &cmd[0];
    let args = &cmd[1..];

    if verbose {
        println!(
            "Running with context '{}': {} {}",
            context_name,
            program,
            args.join(" ")
        );
    }

    let mut command = Command::new(program);
    command.args(args);
    for (k, v) in env {
        match v {
            Some(v) => command.env(k, v),
            None => command.env_remove(k),
        };
    }
    let status = command
        .status()
        .with_context(|| format!("Failed to execute command: {}", program))?;

    Ok(exit_code(status))
}

/// Maps a child's status to the code gcpx should exit with: the child's own
/// code, or 128 + signal number when it was killed by a signal (shell
/// convention).
fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}
