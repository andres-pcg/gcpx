//! Per-shell context activation via env-var exports.
//!
//! The `use` / `unuse` / `auto` modes here print shell-eval'able code that
//! sets `GOOGLE_APPLICATION_CREDENTIALS`, `CLOUDSDK_ACTIVE_CONFIG_NAME`, and
//! `KUBECONFIG` for the *current shell* — no global filesystem mutation, so
//! shells in other terminals are unaffected.

use anyhow::{Result, bail};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::config::{
    context_exists, get_context_adc_path, get_context_kube_path, get_store_dir,
    load_context_metadata, validate_context_name,
};
use crate::workspace::{find_workspace_config, is_trusted_workspace};

#[derive(Clone, Copy)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "zsh" => Ok(Shell::Zsh),
            "bash" => Ok(Shell::Bash),
            "fish" => Ok(Shell::Fish),
            other => bail!("Unsupported shell '{}'. Use zsh, bash, or fish.", other),
        }
    }
}

/// Env-var operations emitted to the shell.
enum Op {
    Set(&'static str, String),
    Unset(&'static str),
}

const MANAGED_VARS: &[&str] = &[
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLOUDSDK_ACTIVE_CONFIG_NAME",
    "KUBECONFIG",
    "GCPX_CONTEXT",
];

fn emit(shell: Shell, ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        match (shell, op) {
            (Shell::Fish, Op::Set(k, v)) => {
                out.push_str(&format!("set -gx {} {};\n", k, shell_quote_fish(v)));
            }
            (Shell::Fish, Op::Unset(k)) => {
                out.push_str(&format!("set -e {};\n", k));
            }
            (_, Op::Set(k, v)) => {
                out.push_str(&format!("export {}={};\n", k, shell_quote_posix(v)));
            }
            (_, Op::Unset(k)) => {
                out.push_str(&format!("unset {};\n", k));
            }
        }
    }
    out
}

fn shell_quote_posix(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

fn shell_quote_fish(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{}'", escaped)
}

/// Builds the Set ops to activate a context. Errors if context is missing.
fn ops_for_context(name: &str) -> Result<Vec<Op>> {
    if !context_exists(name)? {
        bail!(
            "Context '{}' not found. Run 'gcpx save {}' or 'gcpx login {}' first.",
            name,
            name,
            name
        );
    }
    let adc = get_context_adc_path(name)?;
    let metadata = load_context_metadata(name)?;
    let gcloud_config = metadata
        .as_ref()
        .map(|m| m.gcloud_config.clone())
        .unwrap_or_else(|| name.to_string());

    let mut ops = vec![
        Op::Set(
            "GOOGLE_APPLICATION_CREDENTIALS",
            adc.to_string_lossy().into_owned(),
        ),
        Op::Set("CLOUDSDK_ACTIVE_CONFIG_NAME", gcloud_config),
        Op::Set("GCPX_CONTEXT", name.to_string()),
    ];

    let kube = get_context_kube_path(name)?;
    if kube.exists() {
        ops.push(Op::Set("KUBECONFIG", kube.to_string_lossy().into_owned()));
    } else {
        ops.push(Op::Unset("KUBECONFIG"));
    }

    Ok(ops)
}

fn unset_all() -> Vec<Op> {
    let mut ops: Vec<Op> = MANAGED_VARS.iter().map(|k| Op::Unset(k)).collect();
    ops.push(Op::Unset("GCPX_SHELL_PIN"));
    ops.push(Op::Unset("GCPX_SHELL_PIN_FORCE"));
    ops.push(Op::Unset("GCPX_AUTO_APPLIED"));
    ops
}

/// Returns the workspace-pinned context (if any) and whether its directory
/// is trusted for auto-application. The context itself is returned regardless
/// of trust — explicit `gcpx use` can still consult it for the safety guard.
fn workspace_context() -> Result<Option<(String, bool)>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some((dir, wc)) = find_workspace_config(&cwd)? {
        if let Some(ctx) = wc.context {
            let trusted = is_trusted_workspace(&dir);
            return Ok(Some((ctx, trusted)));
        }
    }
    Ok(None)
}

// --- Public entry points (driven by main.rs) ---

/// `gcpx __export use [name] [--force] --shell <shell>`
///
/// Safety rule: when the current directory is under a `.gcpx.toml` with a
/// `context = "..."` pin, a different `gcpx use` is refused unless `--force`
/// is passed. This protects against accidentally overriding the project's
/// declared context (which the auto-switch hook would re-apply on next `cd`
/// anyway, unless force is sticky).
pub fn export_use(name: Option<&str>, force: bool, shell: Shell) -> Result<String> {
    let target = match name {
        Some(n) => {
            validate_context_name(n)?;
            n.to_string()
        }
        None => resolve_target()?
            .ok_or_else(|| anyhow::anyhow!("No context specified and no workspace/default set."))?,
    };

    // Workspace-pin safety guard.
    if let Some((ws_ctx, _trusted)) = workspace_context()? {
        if ws_ctx != target && !force {
            bail!(
                "Workspace .gcpx.toml pins this directory to '{}'. \
                 Refusing to switch to '{}'. \
                 Re-run with `gcpx use {} --force` to override for this shell only.",
                ws_ctx,
                target,
                target
            );
        }
    }

    let mut ops = ops_for_context(&target)?;
    ops.push(Op::Set("GCPX_SHELL_PIN", target.clone()));
    if force {
        ops.push(Op::Set("GCPX_SHELL_PIN_FORCE", "1".to_string()));
    } else {
        ops.push(Op::Unset("GCPX_SHELL_PIN_FORCE"));
    }
    // Clear auto-applied marker — explicit pin overrides
    ops.push(Op::Unset("GCPX_AUTO_APPLIED"));
    Ok(emit(shell, &ops))
}

/// `gcpx __export unuse --shell <shell>` — clear pin, then re-apply auto.
pub fn export_unuse(shell: Shell) -> Result<String> {
    // Drop the shell pin and managed vars, then let auto re-resolve.
    let mut ops = unset_all();

    // If a workspace/default exists, layer it on.
    if let Ok(Some(name)) = resolve_target() {
        if context_exists(&name).unwrap_or(false) {
            let mut applied = ops_for_context(&name)?;
            applied.push(Op::Set("GCPX_AUTO_APPLIED", name));
            ops.extend(applied);
        }
    }
    Ok(emit(shell, &ops))
}

/// `gcpx __export auto --shell <shell>` — chpwd / precmd hook handler.
///
/// Resolution priority:
///   1. GCPX_SHELL_PIN_FORCE=1 with GCPX_SHELL_PIN set → use the pin
///      (only way `gcpx use --force` survives across `cd`s).
///   2. Trusted workspace `.gcpx.toml` → use its context. **This overrides a
///      non-force shell pin** — the project-level safety guard wins.
///   3. GCPX_SHELL_PIN (non-force) → use the pin.
///   4. Global default → use it.
///   5. Otherwise → clear managed env vars.
///
/// Only emits exports when the desired context differs from
/// `GCPX_AUTO_APPLIED` (cheap hook on every `cd`). Prints a one-line
/// stderr notice on actual change so credential swaps are always visible.
pub fn export_auto(shell: Shell) -> Result<String> {
    let pin = env::var("GCPX_SHELL_PIN").ok();
    let pin_forced = env::var("GCPX_SHELL_PIN_FORCE").as_deref() == Ok("1");
    let workspace = workspace_context().unwrap_or(None);

    // Compute desired context per the priority rules above.
    let (desired, override_note): (Option<String>, Option<String>) = if pin_forced && pin.is_some()
    {
        (pin.clone(), None)
    } else if let Some((ws_ctx, trusted)) = workspace.clone() {
        if trusted {
            let note = match &pin {
                Some(p) if *p != ws_ctx => Some(format!(
                    "gcpx: workspace pin '{}' overrides shell pin '{}' (use --force to keep shell pin)",
                    ws_ctx, p
                )),
                _ => None,
            };
            (Some(ws_ctx), note)
        } else {
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if let Some((dir, _)) = find_workspace_config(&cwd).ok().flatten() {
                eprintln!(
                    "gcpx: ignoring .gcpx.toml in untrusted location {:?} \
                     (set GCPX_TRUST_ALL_WORKSPACES=1 to override)",
                    dir
                );
            }
            (pin.clone().or(read_default().unwrap_or(None)), None)
        }
    } else {
        (pin.clone().or(read_default().unwrap_or(None)), None)
    };

    let applied = env::var("GCPX_AUTO_APPLIED").ok();

    match (desired.as_deref(), applied.as_deref()) {
        (Some(d), Some(a)) if d == a => Ok(String::new()),
        (None, None) => Ok(String::new()),
        (Some(d), _) => {
            if !context_exists(d).unwrap_or(false) {
                return Ok(String::new());
            }
            if let Some(note) = override_note {
                eprintln!("{}", note);
            }
            let mut ops = ops_for_context(d)?;
            ops.push(Op::Set("GCPX_AUTO_APPLIED", d.to_string()));
            eprintln!("gcpx: auto-switched to '{}'", d);
            Ok(emit(shell, &ops))
        }
        (None, Some(_)) => {
            let mut ops: Vec<Op> = MANAGED_VARS.iter().map(|k| Op::Unset(k)).collect();
            ops.push(Op::Unset("GCPX_AUTO_APPLIED"));
            eprintln!("gcpx: auto context cleared");
            Ok(emit(shell, &ops))
        }
    }
}

/// Resolution for explicit `gcpx use` with no name — trusts workspace
/// (without trust check; user is asking) then falls back to default.
fn resolve_target() -> Result<Option<String>> {
    if let Some((ctx, _)) = workspace_context()? {
        return Ok(Some(ctx));
    }
    read_default()
}

fn default_path() -> Result<PathBuf> {
    Ok(get_store_dir()?.join(".default"))
}

pub fn read_default() -> Result<Option<String>> {
    let p = default_path()?;
    if !p.exists() {
        return Ok(None);
    }
    let s = fs::read_to_string(p)?.trim().to_string();
    if s.is_empty() {
        return Ok(None);
    }
    // Defense-in-depth: file is normally only written by set_default (which
    // validates), but anyone with write access to ~/.config/gcpx could tamper
    // with it. Reject silently rather than feed an unvalidated string into
    // path-joining or env-var exports.
    if validate_context_name(&s).is_err() {
        eprintln!("gcpx: ignoring invalid default context in .default");
        return Ok(None);
    }
    Ok(Some(s))
}

pub fn set_default(name: &str) -> Result<()> {
    validate_context_name(name)?;
    if !context_exists(name)? {
        bail!("Context '{}' not found.", name);
    }
    fs::write(default_path()?, name)?;
    println!("Default context set to '{}'.", name);
    println!("New shells will use this context unless a .gcpx.toml or `gcpx use` overrides it.");
    Ok(())
}

pub fn clear_default() -> Result<()> {
    let p = default_path()?;
    if p.exists() {
        fs::remove_file(&p)?;
    }
    println!("Default context cleared.");
    Ok(())
}

pub fn show_default() -> Result<()> {
    match read_default()? {
        Some(n) => println!("{}", n),
        None => println!("(no default set)"),
    }
    Ok(())
}
