//! Run command implementation - execute commands with a specific context.

use anyhow::{Context, Result, bail};
use std::process::Command;

use crate::config::{
    context_exists, get_context_adc_path, load_context_metadata, validate_context_name,
};

/// Runs a command with a specific context without switching globally.
///
/// This function sets environment variables to temporarily use the specified
/// context for the subprocess only:
/// - `GOOGLE_APPLICATION_CREDENTIALS`: Points to the context's ADC file
/// - `CLOUDSDK_ACTIVE_CONFIG_NAME`: Sets the gcloud configuration name
///
/// The current shell's context is not affected.
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
pub fn run_with_context(context_name: &str, cmd: &[String], verbose: bool) -> Result<()> {
    validate_context_name(context_name)?;
    if cmd.is_empty() {
        bail!("No command specified. Usage: gcpx run <context> -- <command>");
    }

    if !context_exists(context_name)? {
        bail!(
            "Context '{}' not found. Run 'gcpx save {}' first.",
            context_name,
            context_name
        );
    }

    let adc_path = get_context_adc_path(context_name)?;

    // Load metadata to get the actual gcloud config name
    let metadata = load_context_metadata(context_name)?;
    let gcloud_config = metadata
        .as_ref()
        .map(|m| m.gcloud_config.as_str())
        .unwrap_or(context_name);

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

    let status = Command::new(program)
        .args(args)
        .env("GOOGLE_APPLICATION_CREDENTIALS", &adc_path)
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", gcloud_config)
        .status()
        .with_context(|| format!("Failed to execute command: {}", program))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        bail!("Command exited with code {}", code);
    }

    Ok(())
}
