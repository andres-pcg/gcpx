//! Login command implementation - authenticate and save a context.
//!
//! `gcpx login` with no arguments is the guided first-time setup: it asks for
//! the context name, signs in, lets the user pick a project and offers to make
//! the context the default. Every prompt has a flag equivalent so the command
//! also works non-interactively (`gcpx login <name> --project <id>`).

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, FuzzySelect, Input, theme::ColorfulTheme};
use std::io::IsTerminal;
use std::process::Command;

use crate::auth::parse_ini_value;
use crate::commands::save::save_context;
use crate::commands::use_cmd::{read_default, set_default};
use crate::config::{context_exists, get_gcloud_dir, validate_context_name};

/// Authenticates a new or existing context.
///
/// 1. Resolves the context name (prompted when `name` is `None`)
/// 2. Activates or creates the gcloud configuration of the same name
/// 3. Sets the project up front when `project` is given
/// 4. Runs `gcloud auth login --update-adc` (one browser flow for gcloud + ADC)
/// 5. Offers a project picker when no project is set (interactive only)
/// 6. Saves the context, and offers to make it the default if none is set
///
/// If `quiet` is true, sensitive details are hidden after save.
pub fn login_context(name: Option<&str>, project: Option<&str>, quiet: bool) -> Result<()> {
    let interactive = std::io::stdin().is_terminal();
    let theme = ColorfulTheme::default();

    let name = match name {
        Some(n) => n.to_string(),
        None if interactive => prompt_context_name(&theme)?,
        None => bail!("Context name required: gcpx login <name> [--project <id>]"),
    };
    validate_context_name(&name)?;

    prepare_gcloud_config(&name)?;

    // Setting the project before login lets --update-adc record it as the
    // ADC quota project.
    if let Some(p) = project {
        set_config_project(&name, p)?;
    }

    // One browser flow for both gcloud and ADC: --update-adc writes the same
    // grant to the ADC file, instead of a second `application-default login`.
    println!("\nStarting gcloud authentication (gcloud + ADC)...");
    println!("A browser window will open for you to sign in.\n");

    let auth_status = Command::new("gcloud")
        .args(["auth", "login", "--update-adc"])
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", &name)
        .status()
        .context("Failed to run gcloud auth login")?;

    if !auth_status.success() {
        bail!("gcloud auth login did not complete; nothing was saved.");
    }

    if project.is_none() && interactive && config_project(&name).is_none() {
        if let Some(p) = prompt_project(&theme, &name)? {
            set_config_project(&name, &p)?;
            set_adc_quota_project(&name, &p);
        }
    }

    // Save the context (with kubectl validation)
    println!("\nSaving credentials to context '{}'...", name);
    save_context(&name, quiet, false, false)?;

    if interactive && read_default()?.is_none() {
        let make_default = Confirm::with_theme(&theme)
            .with_prompt(format!(
                "Make '{}' the default context for new shells?",
                name
            ))
            .default(true)
            .interact()?;
        if make_default {
            set_default(&name)?;
        }
    }

    println!("\nContext '{}' is ready.", name);
    println!("  Use it in this shell:   gcpx use {}", name);
    println!(
        "  Pin it to a project:    echo 'context = \"{}\"' > .gcpx.toml",
        name
    );
    Ok(())
}

fn prompt_context_name(theme: &ColorfulTheme) -> Result<String> {
    loop {
        let name: String = Input::with_theme(theme)
            .with_prompt("Context name (e.g. the account or client it's for)")
            .validate_with(|s: &String| validate_context_name(s).map_err(|e| e.to_string()))
            .interact_text()?;
        if !context_exists(&name)? {
            return Ok(name);
        }
        let relogin = Confirm::with_theme(theme)
            .with_prompt(format!(
                "Context '{}' already exists. Sign in again and overwrite it?",
                name
            ))
            .default(false)
            .interact()?;
        if relogin {
            return Ok(name);
        }
    }
}

/// Activates the gcloud configuration named after the context, creating it if
/// needed.
fn prepare_gcloud_config(name: &str) -> Result<()> {
    println!("Setting up gcloud configuration '{}'...", name);

    let check = Command::new("gcloud")
        .args(["config", "configurations", "describe", name])
        .output()
        .context("Failed to execute gcloud command")?;

    let verb = if check.status.success() {
        "activate"
    } else {
        println!("Creating new gcloud configuration '{}'...", name);
        "create"
    };
    let status = Command::new("gcloud")
        .args(["config", "configurations", verb, name])
        .status()
        .with_context(|| format!("Failed to {} gcloud configuration", verb))?;
    if !status.success() {
        println!("Warning: Could not {} configuration '{}'", verb, name);
    }
    Ok(())
}

/// Reads `core/project` from the context's gcloud configuration file.
fn config_project(name: &str) -> Option<String> {
    let path = get_gcloud_dir()
        .ok()?
        .join("configurations")
        .join(format!("config_{}", name));
    let content = std::fs::read_to_string(path).ok()?;
    parse_ini_value(&content, "core", "project")
}

fn set_config_project(name: &str, project: &str) -> Result<()> {
    let status = Command::new("gcloud")
        .args(["config", "set", "project", project, "--quiet"])
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", name)
        .status()
        .context("Failed to set gcloud project")?;
    if !status.success() {
        bail!(
            "Could not set project '{}' on configuration '{}'",
            project,
            name
        );
    }
    Ok(())
}

/// Records the project as ADC quota project. gcloud checks the user may bill
/// usage to it; if not, ADC simply stays without a quota project.
fn set_adc_quota_project(name: &str, project: &str) {
    let ok = Command::new("gcloud")
        .args(["auth", "application-default", "set-quota-project", project])
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", name)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!(
            "Note: '{}' was not set as the ADC quota project; client libraries will use their defaults.",
            project
        );
    }
}

/// Lists the projects the signed-in account can see and lets the user pick
/// one, type one, or skip.
fn prompt_project(theme: &ColorfulTheme, name: &str) -> Result<Option<String>> {
    println!("\nFetching your projects...");
    let projects: Vec<String> = Command::new("gcloud")
        .args([
            "projects",
            "list",
            "--format=value(projectId)",
            "--sort-by=projectId",
        ])
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    const TYPE_IT: &str = "[enter a project ID]";
    const SKIP: &str = "[skip — no default project]";
    let mut items: Vec<&str> = projects.iter().map(String::as_str).collect();
    items.push(TYPE_IT);
    items.push(SKIP);

    let idx = FuzzySelect::with_theme(theme)
        .with_prompt("Default project for this context (type to search)")
        .items(&items)
        .default(0)
        .interact()?;

    match items[idx] {
        SKIP => Ok(None),
        TYPE_IT => {
            let p: String = Input::with_theme(theme)
                .with_prompt("Project ID")
                .interact_text()?;
            let p = p.trim().to_string();
            Ok((!p.is_empty()).then_some(p))
        }
        p => Ok(Some(p.to_string())),
    }
}
