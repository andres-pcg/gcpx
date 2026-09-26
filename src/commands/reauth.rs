//! Reauth command — refresh expired sessions for saved contexts.
//!
//! gcloud credentials: `gcloud auth login <account>` is run inside the
//! context's gcloud configuration. When the organisation allows it, gcloud
//! re-authenticates with a password prompt in the terminal; otherwise it falls
//! back to the browser.
//!
//! ADC (opt-in via `--adc`): a password reauth only yields a RAPT that must be
//! sent on every refresh, which most client libraries (e.g. Terraform's) don't
//! do. So ADC always gets a fresh browser grant, obtained in the *same* flow as
//! gcloud's via `--update-adc` — one browser trip per context.

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::Command;
use std::time::Duration;

use crate::auth::{
    ContextHealth, CredState, adoptable_global_adc, check_adc_bytes, check_gcloud_account,
    read_config_account, read_optional, same_account,
};
use crate::commands::save::sanitize_for_display;
use crate::config::{
    get_adc_path, get_context_adc_path, get_current_tracking, list_contexts, validate_context_name,
    write_secret,
};

#[derive(Debug, Clone, Copy)]
pub struct ReauthOptions {
    /// Also refresh the context's ADC.
    pub adc: bool,
    /// Re-login even if credentials look valid.
    pub force: bool,
}

/// `gcpx reauth [names...] [--all] [--adc] [--force]`.
pub fn reauth(names: &[String], all: bool, opts: ReauthOptions) -> Result<()> {
    let explicit = !names.is_empty();
    let targets: Vec<String> = if explicit {
        for n in names {
            validate_context_name(n)?;
        }
        names.to_vec()
    } else if all {
        list_contexts()?
    } else {
        bail!("Specify a context name, or --all.");
    };

    let checks = crate::auth::check_contexts(&targets, Duration::from_secs(15));
    let mut failures = Vec::new();
    let mut done = 0;
    let mut run = RunState::default();

    for (name, check) in targets.iter().zip(checks) {
        let health = match check {
            Ok(h) => h,
            Err(e) => {
                eprintln!("{}: {}", name, e);
                failures.push(name.clone());
                continue;
            }
        };
        let Some(account) = health.expected_account.clone() else {
            // Contexts without a saved account can't be reauthenticated
            // safely — we'd have no way to confirm who signed in.
            if explicit {
                eprintln!(
                    "{}: no account saved for this context; run `gcpx login {}` instead.",
                    name, name
                );
                failures.push(name.clone());
            }
            continue;
        };

        let mut plan = Plan::from(&health, opts);
        // gcloud credentials are per account: several contexts sharing one
        // account need a single sign-in per run.
        let covered = !opts.force && run.gcloud_refreshed.contains(&account.to_lowercase());
        if covered {
            plan.gcloud = false;
        }
        if !plan.anything() {
            if covered {
                println!(
                    "{}: covered by an earlier sign-in for {} in this run.",
                    name,
                    sanitize_for_display(&account)
                );
                continue;
            }
            if explicit {
                println!(
                    "{}: already valid ({}).",
                    name,
                    sanitize_for_display(&account)
                );
            }
            continue;
        }

        done += 1;
        println!("\n== {} ({}) ==", name, sanitize_for_display(&account));
        if let Err(e) = reauth_one(&health, &account, plan, opts, &mut run) {
            eprintln!("{}: {:#}", name, e);
            failures.push(name.clone());
        }
    }

    if !explicit && done == 0 && failures.is_empty() {
        println!(
            "Nothing to do — all {} are valid.",
            if opts.adc {
                "sessions and ADC"
            } else {
                "gcloud sessions"
            }
        );
    }
    if !failures.is_empty() {
        bail!("Reauth failed for: {}", failures.join(", "));
    }
    Ok(())
}

/// What this run has already refreshed, keyed by lowercase account.
#[derive(Default)]
struct RunState {
    gcloud_refreshed: HashSet<String>,
    fresh_adc: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
struct Plan {
    gcloud: bool,
    fix_config: bool,
    adc: bool,
}

impl Plan {
    fn from(h: &ContextHealth, opts: ReauthOptions) -> Self {
        Plan {
            gcloud: opts.force || h.gcloud_stale(),
            fix_config: h.config_mismatch(),
            adc: opts.adc && (opts.force || h.adc_stale() || h.adc_mismatch()),
        }
    }
    fn anything(&self) -> bool {
        self.gcloud || self.fix_config || self.adc
    }
}

fn reauth_one(
    h: &ContextHealth,
    account: &str,
    mut plan: Plan,
    opts: ReauthOptions,
    run: &mut RunState,
) -> Result<()> {
    let name = h.name.as_str();
    let cfg = h.gcloud_config.as_str();

    if plan.fix_config {
        let current = h.config_account.as_deref().unwrap_or("?");
        println!(
            "gcloud config '{}' is set to {}, but '{}' was saved for {}.",
            cfg,
            sanitize_for_display(current),
            name,
            sanitize_for_display(account)
        );
        let ok = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Sign in as {} and point config '{}' back to it?",
                sanitize_for_display(account),
                cfg
            ))
            .default(true)
            .interact()?;
        if !ok {
            bail!("skipped (account mismatch left as is)");
        }
    }

    // A stale ADC whose working replacement is already on disk (e.g. the user
    // ran `gcloud auth application-default login` directly) is adopted without
    // a browser — but only when Google confirms it's the same account.
    let ctx_adc = get_context_adc_path(name)?;
    let key = account.to_lowercase();
    if plan.adc {
        if let Some(fresh) = run.fresh_adc.get(&key) {
            let old = read_optional(&ctx_adc)?;
            write_secret(&ctx_adc, &with_quota_project_of(fresh, old.as_deref()))?;
            println!(
                "ADC: reused this run's new credential for {}.",
                sanitize_for_display(account)
            );
            plan.adc = false;
        }
    }
    if plan.adc && !opts.force {
        if let Some(bytes) = adoptable_global_adc(account, &ctx_adc)? {
            let old = read_optional(&ctx_adc)?;
            write_secret(&ctx_adc, &with_quota_project_of(&bytes, old.as_deref()))?;
            println!(
                "ADC: adopted a working credential for {} from gcloud's ADC file.",
                sanitize_for_display(account)
            );
            plan.adc = false;
        }
    }

    if plan.adc {
        let fresh = login_with_adc(name, cfg, account, &ctx_adc)?;
        run.fresh_adc.insert(key.clone(), fresh);
        run.gcloud_refreshed.insert(key);
    } else if plan.gcloud || plan.fix_config {
        run_gcloud_login(cfg, account, opts.force, false)?;
        run.gcloud_refreshed.insert(key);
    }

    verify_gcloud(cfg, account)?;
    println!("✓ {} is ready.", name);
    Ok(())
}

/// Runs `gcloud auth login <account>` scoped to the context's configuration
/// (so it never changes which configuration is active globally).
fn run_gcloud_login(cfg: &str, account: &str, force: bool, update_adc: bool) -> Result<()> {
    let mut args = vec!["auth", "login", account];
    if force {
        args.push("--force");
    }
    if update_adc {
        args.push("--update-adc");
    }
    let status = Command::new("gcloud")
        .args(&args)
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", cfg)
        .env_remove("CLOUDSDK_CORE_ACCOUNT")
        .status()
        .context("Failed to run gcloud auth login")?;
    if !status.success() {
        bail!("gcloud auth login did not complete");
    }
    Ok(())
}

/// One browser trip that refreshes both gcloud's credential and the context's
/// ADC. gcloud writes ADC to its well-known path; we capture it into the
/// context and then put the previous global ADC back, so other shells and
/// contexts are unaffected.
fn login_with_adc(
    name: &str,
    cfg: &str,
    account: &str,
    ctx_adc: &std::path::Path,
) -> Result<Vec<u8>> {
    let global = get_adc_path()?;
    let backup = read_optional(&global)?;

    let result = (|| -> Result<Vec<u8>> {
        // --force: a password-only reauth would give ADC a RAPT-bound
        // credential that most client libraries can't refresh.
        run_gcloud_login(cfg, account, true, true)?;
        let fresh = fs::read(&global).context("gcloud did not write ADC")?;
        let check = check_adc_bytes(&fresh);
        match (&check.state, check.email.as_deref()) {
            (CredState::Valid, Some(e)) if same_account(e, account) => {}
            (CredState::Valid, Some(e)) => bail!(
                "new ADC belongs to {}, not {}; not saved",
                sanitize_for_display(e),
                sanitize_for_display(account)
            ),
            (CredState::Valid, None) => {
                bail!("could not confirm which account the new ADC belongs to; not saved")
            }
            (s, _) => bail!("new ADC does not work ({}); not saved", s.label()),
        }
        write_secret(ctx_adc, &fresh)?;
        println!("ADC: saved to context '{}'.", name);
        Ok(fresh)
    })();

    // Legacy `gcpx switch` users expect the global ADC to match the active
    // context; keep the fresh one in that case, restore otherwise.
    let keep_fresh = result.is_ok() && get_current_tracking() == name;
    if !keep_fresh {
        match backup {
            Some(b) => write_secret(&global, &b)?,
            None => {
                let _ = fs::remove_file(&global);
            }
        }
    }
    result
}

fn verify_gcloud(cfg: &str, account: &str) -> Result<()> {
    let now = read_config_account(cfg);
    if !now.as_deref().is_some_and(|a| same_account(a, account)) {
        bail!(
            "gcloud config '{}' now points at {} instead of {}",
            cfg,
            sanitize_for_display(now.as_deref().unwrap_or("(unset)")),
            sanitize_for_display(account)
        );
    }
    match check_gcloud_account(account, Duration::from_secs(15)) {
        CredState::Valid => Ok(()),
        s => bail!("gcloud credential still not usable: {}", s.label()),
    }
}

/// Returns `fresh` ADC JSON with the quota project taken from the context's
/// previous ADC (quota projects are per context, credentials per account).
fn with_quota_project_of(fresh: &[u8], old: Option<&[u8]>) -> Vec<u8> {
    const KEY: &str = "quota_project_id";
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(fresh) else {
        return fresh.to_vec();
    };
    let old_quota = old
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(o).ok())
        .and_then(|o| o.get(KEY).cloned());
    if let Some(obj) = v.as_object_mut() {
        match old_quota {
            Some(q) => {
                obj.insert(KEY.into(), q);
            }
            None => {
                obj.remove(KEY);
            }
        }
    }
    serde_json::to_vec_pretty(&v).unwrap_or_else(|_| fresh.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_project_follows_the_context() {
        let fresh = br#"{"type":"authorized_user","refresh_token":"r","quota_project_id":"a"}"#;
        let old = br#"{"quota_project_id":"b"}"#;
        let out: serde_json::Value =
            serde_json::from_slice(&with_quota_project_of(fresh, Some(old))).unwrap();
        assert_eq!(out["quota_project_id"], "b");
        assert_eq!(out["refresh_token"], "r");

        let out: serde_json::Value =
            serde_json::from_slice(&with_quota_project_of(fresh, None)).unwrap();
        assert!(out.get("quota_project_id").is_none());
    }
}
