//! Reauth command — refresh expired sessions for saved contexts.
//!
//! gcloud credentials: `gcloud auth login <account>` is run inside the
//! context's gcloud configuration. When the organisation allows it, gcloud
//! re-authenticates with a password prompt in the terminal; otherwise it opens
//! the browser (or prints a link when it can't).
//!
//! ADC (opt-in via `--adc`): a password reauth only yields a RAPT that must be
//! sent on every refresh, which most client libraries (e.g. Terraform's) don't
//! do. So ADC always gets a fresh browser grant, obtained in the *same* flow as
//! gcloud's via `--update-adc` — one browser trip per context.
//!
//! Two-step mode (`--start` / `--code`, Unix only) is for callers that can't
//! type into a running process, such as AI agents: `--start` leaves gcloud's
//! link-and-code sign-in waiting in the background and returns the link;
//! `--code` hands it the verification code the user got in the browser.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::auth::{
    ContextHealth, CredState, adoptable_global_adc, check_adc_bytes, check_context,
    check_gcloud_account, read_config_account, read_optional, same_account,
};
use crate::commands::save::sanitize_for_display;
use crate::config::{
    ensure_dir_0700, get_adc_path, get_context_adc_path, get_current_tracking, get_store_dir,
    list_contexts, validate_context_name, write_secret,
};
use crate::prompt;

#[derive(Debug, Clone, Copy)]
pub struct ReauthOptions {
    /// Also refresh the context's ADC.
    pub adc: bool,
    /// Re-login even if credentials look valid.
    pub force: bool,
    /// Machine-readable output on stdout; progress goes to stderr.
    pub json: bool,
}

/// Progress output: stdout normally, stderr in `--json` mode so stdout stays
/// a single JSON document.
struct Out {
    json: bool,
}

impl Out {
    fn say(&self, msg: impl AsRef<str>) {
        if self.json {
            eprintln!("{}", msg.as_ref());
        } else {
            println!("{}", msg.as_ref());
        }
    }
}

/// `gcpx reauth [names...] [--all] [--adc] [--force] [--json]`.
pub fn reauth(names: &[String], all: bool, opts: ReauthOptions) -> Result<()> {
    let out = Out { json: opts.json };
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
    let mut results = Vec::new();
    let mut done = 0;
    let mut run = RunState::default();

    for (name, check) in targets.iter().zip(checks) {
        let health = match check {
            Ok(h) => h,
            Err(e) => {
                out.say(format!("{}: {}", name, e));
                results.push(json!({"context": name, "result": "failed", "error": e.to_string()}));
                failures.push(name.clone());
                continue;
            }
        };
        let Some(account) = health.expected_account.clone() else {
            // Contexts without a saved account can't be reauthenticated
            // safely — we'd have no way to confirm who signed in.
            if explicit {
                let msg = format!(
                    "no account saved for this context; run `gcpx login {}` instead",
                    name
                );
                out.say(format!("{}: {}.", name, msg));
                results.push(json!({"context": name, "result": "failed", "error": msg}));
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
            let result = if covered { "covered" } else { "already_valid" };
            if covered {
                out.say(format!(
                    "{}: covered by an earlier sign-in for {} in this run.",
                    name,
                    sanitize_for_display(&account)
                ));
            } else if explicit {
                out.say(format!(
                    "{}: already valid ({}).",
                    name,
                    sanitize_for_display(&account)
                ));
            }
            results.push(json!({"context": name, "account": account, "result": result}));
            continue;
        }

        done += 1;
        out.say(format!(
            "\n== {} ({}) ==",
            name,
            sanitize_for_display(&account)
        ));
        match reauth_one(&health, &account, plan, opts, &mut run, &out) {
            Ok(()) => {
                results.push(json!({"context": name, "account": account, "result": "refreshed"}))
            }
            Err(e) => {
                out.say(format!("{}: {:#}", name, e));
                results.push(json!({"context": name, "account": account, "result": "failed", "error": format!("{:#}", e)}));
                failures.push(name.clone());
            }
        }
    }

    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "results": results }))?
        );
    } else if !explicit && done == 0 && failures.is_empty() {
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

/// Asks before pointing a context's gcloud configuration back at its saved
/// account (the configuration currently names a different one).
fn confirm_config_fix(h: &ContextHealth, account: &str, out: &Out) -> Result<()> {
    let current = h.config_account.as_deref().unwrap_or("?");
    out.say(format!(
        "gcloud config '{}' is set to {}, but '{}' was saved for {}.",
        h.gcloud_config,
        sanitize_for_display(current),
        h.name,
        sanitize_for_display(account)
    ));
    let ok = prompt::confirm(
        &format!(
            "Sign in as {} and point config '{}' back to it?",
            sanitize_for_display(account),
            h.gcloud_config
        ),
        true,
        "Re-run with --yes to point it back to the context's saved account.",
    )?;
    if !ok {
        bail!("skipped (account mismatch left as is)");
    }
    Ok(())
}

fn reauth_one(
    h: &ContextHealth,
    account: &str,
    mut plan: Plan,
    opts: ReauthOptions,
    run: &mut RunState,
    out: &Out,
) -> Result<()> {
    let name = h.name.as_str();
    let cfg = h.gcloud_config.as_str();

    if plan.fix_config {
        confirm_config_fix(h, account, out)?;
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
            out.say(format!(
                "ADC: reused this run's new credential for {}.",
                sanitize_for_display(account)
            ));
            plan.adc = false;
        }
    }
    if plan.adc && !opts.force {
        if let Some(bytes) = adoptable_global_adc(account, &ctx_adc)? {
            let old = read_optional(&ctx_adc)?;
            write_secret(&ctx_adc, &with_quota_project_of(&bytes, old.as_deref()))?;
            out.say(format!(
                "ADC: adopted a working credential for {} from gcloud's ADC file.",
                sanitize_for_display(account)
            ));
            plan.adc = false;
        }
    }

    if plan.adc {
        let fresh = login_with_adc(name, cfg, account, &ctx_adc, out)?;
        run.fresh_adc.insert(key.clone(), fresh);
        run.gcloud_refreshed.insert(key);
    } else if plan.gcloud || plan.fix_config {
        run_gcloud_login(cfg, account, opts.force, false, out)?;
        run.gcloud_refreshed.insert(key);
    }

    verify_gcloud(cfg, account)?;
    out.say(format!("✓ {} is ready.", name));
    Ok(())
}

/// Runs `gcloud auth login <account>` scoped to the context's configuration
/// (so it never changes which configuration is active globally).
///
/// Without a terminal gcloud can't ask for a password, so the browser flow is
/// forced. If gcloud can't open a browser either, it would wait for a code on
/// stdin that nobody can type — the error then points at `--start`/`--code`.
fn run_gcloud_login(
    cfg: &str,
    account: &str,
    force: bool,
    update_adc: bool,
    out: &Out,
) -> Result<()> {
    let interactive = prompt::is_interactive();
    let mut args = vec!["auth", "login", account];
    if force || !interactive {
        args.push("--force");
    }
    if update_adc {
        args.push("--update-adc");
    }
    let mut cmd = Command::new("gcloud");
    cmd.args(&args)
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", cfg)
        .env_remove("CLOUDSDK_CORE_ACCOUNT");
    if !interactive {
        cmd.stdin(Stdio::null());
    }
    if out.json {
        cmd.stdout(Stdio::from(std::io::stderr()));
    }
    let status = cmd.status().context("Failed to run gcloud auth login")?;
    if !status.success() {
        if interactive {
            bail!("gcloud auth login did not complete");
        }
        bail!(
            "gcloud auth login did not complete (no terminal). If no browser could be opened, \
             use the two-step flow: `gcpx reauth <context> --start`, then `--code <CODE>`."
        );
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
    ctx_adc: &Path,
    out: &Out,
) -> Result<Vec<u8>> {
    let backup = read_optional(&get_adc_path()?)?;
    // --force: a password-only reauth would give ADC a RAPT-bound credential
    // that most client libraries can't refresh.
    let result = run_gcloud_login(cfg, account, true, true, out)
        .and_then(|_| capture_fresh_adc(name, account, ctx_adc, out));
    restore_global_adc(name, backup, result.is_ok())?;
    result
}

/// Verifies the ADC gcloud just wrote belongs to `account` and works, then
/// stores it in the context.
fn capture_fresh_adc(name: &str, account: &str, ctx_adc: &Path, out: &Out) -> Result<Vec<u8>> {
    let fresh = fs::read(get_adc_path()?).context("gcloud did not write ADC")?;
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
    let old = read_optional(ctx_adc)?;
    let stored = if old.is_some() {
        with_quota_project_of(&fresh, old.as_deref())
    } else {
        fresh.clone()
    };
    write_secret(ctx_adc, &stored)?;
    out.say(format!("ADC: saved to context '{}'.", name));
    Ok(fresh)
}

/// Puts the previous global ADC back. Legacy `gcpx switch` users expect the
/// global ADC to match the active context, so a successful refresh of the
/// active context is kept.
fn restore_global_adc(name: &str, backup: Option<Vec<u8>>, succeeded: bool) -> Result<()> {
    if succeeded && get_current_tracking() == name {
        return Ok(());
    }
    let global = get_adc_path()?;
    match backup {
        Some(b) => write_secret(&global, &b)?,
        None => {
            let _ = fs::remove_file(&global);
        }
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// Two-step sign-in (`--start` / `--code` / `--cancel`)
// ---------------------------------------------------------------------------

/// How long a started sign-in waits for its code.
const PENDING_TTL: Duration = Duration::from_secs(600);

/// State of a started sign-in, persisted in the context's pending directory.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingState {
    context: String,
    account: String,
    gcloud_config: String,
    adc: bool,
    started_at: u64,
    worker_pid: Option<u32>,
}

fn pending_dir(name: &str) -> Result<PathBuf> {
    Ok(get_store_dir()?.join(".pending").join(name))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_pending(dir: &Path) -> Option<PendingState> {
    serde_json::from_slice(&fs::read(dir.join("state.json")).ok()?).ok()
}

/// Extracts the sign-in URL gcloud printed. Pure; unit-tested.
pub fn find_auth_url(log: &str) -> Option<String> {
    log.split_whitespace()
        .find(|w| w.starts_with("https://accounts.google.com/o/oauth2/auth?"))
        .map(|w| w.to_string())
}

/// Last meaningful gcloud error line, for reporting. Pure; unit-tested.
pub fn last_gcloud_error(log: &str) -> Option<String> {
    log.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("ERROR:"))
        .map(|l| l.chars().take(200).collect())
}

/// `gcpx reauth <name> --start`: begins a link-and-code sign-in and returns
/// the link. The sign-in waits in the background for `--code`.
#[cfg(unix)]
pub fn start(name: &str, opts: ReauthOptions) -> Result<()> {
    let out = Out { json: opts.json };
    validate_context_name(name)?;
    let health = check_context(name, Duration::from_secs(15))?;
    let Some(account) = health.expected_account.clone() else {
        bail!(
            "no account saved for '{}'; run `gcpx login {}` instead",
            name,
            name
        );
    };
    if health.config_mismatch() {
        confirm_config_fix(&health, &account, &out)?;
    }

    let dir = pending_dir(name)?;
    if let Some(state) = load_pending(&dir) {
        if state.worker_pid.is_some_and(process_alive) {
            bail!(
                "A sign-in for '{}' is already waiting. Finish it with `gcpx reauth {} --code <CODE>` \
                 or discard it with `gcpx reauth {} --cancel`.",
                name,
                name,
                name
            );
        }
    }
    let _ = fs::remove_dir_all(&dir);
    ensure_dir_0700(&dir)?;
    if let Some(parent) = dir.parent() {
        ensure_dir_0700(parent)?;
    }

    if opts.adc {
        if let Some(b) = read_optional(&get_adc_path()?)? {
            write_secret(&dir.join("adc.backup"), &b)?;
        }
    }
    let fifo = dir.join("code.fifo");
    let status = Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&fifo)
        .status()
        .context("Failed to run mkfifo")?;
    if !status.success() {
        bail!("Could not create {:?}", fifo);
    }

    let mut state = PendingState {
        context: name.to_string(),
        account: account.clone(),
        gcloud_config: health.gcloud_config.clone(),
        adc: opts.adc,
        started_at: now_secs(),
        worker_pid: None,
    };
    fs::write(dir.join("state.json"), serde_json::to_vec(&state)?)?;

    use std::os::unix::process::CommandExt;
    let worker = Command::new(std::env::current_exe()?)
        .args(["__reauth-worker", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Failed to start background sign-in")?;
    state.worker_pid = Some(worker.id());
    fs::write(dir.join("state.json"), serde_json::to_vec(&state)?)?;

    // Wait for gcloud to print the link.
    let log_path = dir.join("gcloud.log");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let url = loop {
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(url) = find_auth_url(&log) {
            break url;
        }
        let finished = dir.join("result.json").exists() || !process_alive(worker.id());
        if finished || std::time::Instant::now() > deadline {
            cancel_pending(name)?;
            bail!(
                "gcloud did not produce a sign-in link{}",
                last_gcloud_error(&log)
                    .map(|e| format!(": {}", e))
                    .unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    let next = format!("gcpx reauth {} --code <CODE>", name);
    if opts.json {
        let v = json!({
            "context": name,
            "account": account,
            "url": url,
            "expires_in": PENDING_TTL.as_secs(),
            "next": next,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "Sign in as {} by opening this link in a browser:\n\n    {}\n",
            sanitize_for_display(&account),
            url
        );
        println!(
            "Then paste the verification code Google shows you:\n\n    {}\n",
            next
        );
        println!(
            "The link expires in {} minutes.",
            PENDING_TTL.as_secs() / 60
        );
    }
    Ok(())
}

/// `gcpx reauth <name> --code <CODE>`: completes a started sign-in.
/// `code` may be `-` to read it from stdin (keeps it out of the process list).
#[cfg(unix)]
pub fn complete(name: &str, code: &str, opts: ReauthOptions) -> Result<()> {
    let out = Out { json: opts.json };
    validate_context_name(name)?;
    let dir = pending_dir(name)?;
    let Some(state) = load_pending(&dir) else {
        bail!(
            "No sign-in is waiting for '{}'. Start one with `gcpx reauth {} --start`.",
            name,
            name
        );
    };

    let code = if code == "-" {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        s
    } else {
        code.to_string()
    };
    let code = code.trim();
    if code.is_empty() || code.contains(char::is_whitespace) {
        bail!("That doesn't look like a verification code.");
    }

    let backup = read_optional(&dir.join("adc.backup"))?;
    let result = (|| -> Result<()> {
        let mut fifo = open_fifo_for_write(&dir.join("code.fifo")).map_err(|_| {
            anyhow::anyhow!(
                "The sign-in for '{}' expired or was cancelled. Start again with `gcpx reauth {} --start`.",
                name,
                name
            )
        })?;
        writeln!(fifo, "{}", code)?;
        drop(fifo);

        let result_path = dir.join("result.json");
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let exit_code = loop {
            if let Ok(bytes) = fs::read(&result_path) {
                let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
                break v.get("exit_code").and_then(Value::as_i64).unwrap_or(1);
            }
            if std::time::Instant::now() > deadline {
                bail!("gcloud did not finish the sign-in in time");
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if exit_code != 0 {
            let log = fs::read_to_string(dir.join("gcloud.log")).unwrap_or_default();
            bail!(
                "sign-in failed{}. Start again with `gcpx reauth {} --start`.",
                last_gcloud_error(&log)
                    .map(|e| format!(": {}", e))
                    .unwrap_or_default(),
                name
            );
        }
        if state.adc {
            capture_fresh_adc(name, &state.account, &get_context_adc_path(name)?, &out)?;
        }
        verify_gcloud(&state.gcloud_config, &state.account)
    })();

    if state.adc {
        restore_global_adc(name, backup, result.is_ok())?;
    }
    let _ = fs::remove_dir_all(&dir);
    result?;

    if opts.json {
        let v = json!({
            "context": name,
            "account": state.account,
            "result": "refreshed",
            "adc": state.adc,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        out.say(format!("✓ {} is ready.", name));
    }
    Ok(())
}

/// `gcpx reauth <name> --cancel`: discards a started sign-in.
#[cfg(unix)]
pub fn cancel(name: &str, opts: ReauthOptions) -> Result<()> {
    validate_context_name(name)?;
    let had = cancel_pending(name)?;
    if opts.json {
        println!("{}", json!({"context": name, "cancelled": had}));
    } else if had {
        println!("Cancelled the waiting sign-in for '{}'.", name);
    } else {
        println!("No sign-in was waiting for '{}'.", name);
    }
    Ok(())
}

#[cfg(unix)]
fn cancel_pending(name: &str) -> Result<bool> {
    let dir = pending_dir(name)?;
    let Some(state) = load_pending(&dir) else {
        let _ = fs::remove_dir_all(&dir);
        return Ok(false);
    };
    if let Some(pid) = state.worker_pid {
        if process_alive(pid) {
            // SAFETY: plain kill(2) on the worker's process group, which only
            // contains the worker and the gcloud it started.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }
    if state.adc {
        restore_global_adc(name, read_optional(&dir.join("adc.backup"))?, false)?;
    }
    let _ = fs::remove_dir_all(&dir);
    Ok(true)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Opens the FIFO for writing without blocking; fails if nobody is reading
/// (the worker is gone).
#[cfg(unix)]
fn open_fifo_for_write(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Hidden `gcpx __reauth-worker <name>`: runs gcloud's link-and-code sign-in,
/// feeds it the code that arrives through the FIFO, records the exit code.
/// Gives up (and kills gcloud) after [`PENDING_TTL`].
#[cfg(unix)]
pub fn worker(name: &str) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;

    validate_context_name(name)?;
    let dir = pending_dir(name)?;
    let state = load_pending(&dir).context("no pending state")?;
    let log = fs::File::create(dir.join("gcloud.log"))?;

    let mut args = vec![
        "auth".to_string(),
        "login".to_string(),
        state.account.clone(),
        "--force".to_string(),
        "--no-launch-browser".to_string(),
    ];
    if state.adc {
        args.push("--update-adc".to_string());
    }
    let mut gcloud = Command::new("gcloud")
        .args(&args)
        .env("CLOUDSDK_ACTIVE_CONFIG_NAME", &state.gcloud_config)
        .env_remove("CLOUDSDK_CORE_ACCOUNT")
        .stdin(Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("Failed to run gcloud")?;

    let fifo = dir.join("code.fifo");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Blocks until `--code` opens the FIFO for writing.
        let line = fs::File::open(&fifo).ok().and_then(|f| {
            let mut s = String::new();
            BufReader::new(f).read_line(&mut s).ok().map(|_| s)
        });
        let _ = tx.send(line);
    });

    // Wait for the code, but stop early if gcloud exits on its own (e.g. it
    // failed before printing the link).
    let deadline = std::time::Instant::now() + PENDING_TTL;
    let code = loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => break line,
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Ok(Some(status)) = gcloud.try_wait() {
            fs::write(
                dir.join("result.json"),
                serde_json::to_vec(&json!({ "exit_code": status.code().unwrap_or(1) }))?,
            )?;
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            break None;
        }
    };
    let exit_code = match code {
        Some(code) => {
            if let Some(mut stdin) = gcloud.stdin.take() {
                let _ = stdin.write_all(code.as_bytes());
            }
            gcloud.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1)
        }
        None => {
            let _ = gcloud.kill();
            let _ = gcloud.wait();
            if state.adc {
                restore_global_adc(name, read_optional(&dir.join("adc.backup"))?, false)?;
            }
            let _ = fs::remove_dir_all(&dir);
            return Ok(());
        }
    };
    fs::write(
        dir.join("result.json"),
        serde_json::to_vec(&json!({ "exit_code": exit_code }))?,
    )?;
    Ok(())
}

#[cfg(not(unix))]
pub fn start(_: &str, _: ReauthOptions) -> Result<()> {
    bail!("The two-step sign-in (--start/--code) is only supported on macOS and Linux.")
}
#[cfg(not(unix))]
pub fn complete(_: &str, _: &str, _: ReauthOptions) -> Result<()> {
    bail!("The two-step sign-in (--start/--code) is only supported on macOS and Linux.")
}
#[cfg(not(unix))]
pub fn cancel(_: &str, _: ReauthOptions) -> Result<()> {
    bail!("The two-step sign-in (--start/--code) is only supported on macOS and Linux.")
}
#[cfg(not(unix))]
pub fn worker(_: &str) -> Result<()> {
    bail!("unsupported platform")
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

    #[test]
    fn finds_auth_url_in_gcloud_output() {
        let log = "Go to the following link in your browser, and complete the sign-in prompts:\n\n    https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=x\n\nOnce finished, enter the verification code provided in your browser: ";
        assert_eq!(
            find_auth_url(log).as_deref(),
            Some("https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=x")
        );
        assert_eq!(find_auth_url("nothing here"), None);
    }

    #[test]
    fn extracts_last_gcloud_error() {
        let log = "Once finished...: ERROR: There was a problem with web authentication.\nERROR: (gcloud.auth.login) (invalid_grant) Malformed auth code.\n";
        assert_eq!(
            last_gcloud_error(log).as_deref(),
            Some("ERROR: (gcloud.auth.login) (invalid_grant) Malformed auth code.")
        );
    }
}
