//! Interactive prompts that degrade cleanly without a terminal.
//!
//! gcpx is used both by people and by automation (scripts, CI, AI agents).
//! Every prompt goes through here so that, without a TTY, gcpx never blocks or
//! fails with a cryptic "not a terminal" error: it either applies the safe
//! default (when the global `--yes` flag is set) or explains which flag to use.

use anyhow::{Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

static ASSUME_YES: AtomicBool = AtomicBool::new(false);

/// Set from the global `--yes` flag.
pub fn set_assume_yes(v: bool) {
    ASSUME_YES.store(v, Ordering::Relaxed);
}

pub fn assume_yes() -> bool {
    ASSUME_YES.load(Ordering::Relaxed)
}

/// True when prompts can be shown (stdin and stderr are terminals).
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Yes/no question. With `--yes` the answer is `default`. Without a terminal
/// (and without `--yes`) it errors with `hint`, which should name the flag(s)
/// that make the command non-interactive.
pub fn confirm(question: &str, default: bool, hint: &str) -> Result<bool> {
    if assume_yes() {
        return Ok(default);
    }
    if !is_interactive() {
        bail!(
            "Needs confirmation: {}\nNo terminal to ask on. {}",
            question,
            hint
        );
    }
    Ok(Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(question)
        .default(default)
        .interact()?)
}

/// Errors when a prompt with no safe default would be needed and there is no
/// terminal.
pub fn require_interactive(what: &str, hint: &str) -> Result<()> {
    if !is_interactive() {
        bail!("{} needs a terminal. {}", what, hint);
    }
    Ok(())
}
