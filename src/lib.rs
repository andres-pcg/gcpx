//! # gcpx - GCP Context Switcher
//!
//! A CLI tool for managing multiple Google Cloud Platform accounts with
//! seamless switching of both gcloud configurations and ADC credentials.
//!
//! ## Problem
//!
//! Managing multiple GCP accounts requires constant re-authentication when
//! switching between accounts, especially for ADC (Application Default
//! Credentials) which are stored in a single file.
//!
//! ## Solution
//!
//! `gcpx` saves separate ADC credentials for each account and swaps them
//! automatically when switching, eliminating the need for repeated
//! `gcloud auth application-default login`.

pub mod commands;
pub mod config;

// Re-export commonly used items
pub use commands::{
    delete_context, interactive_switch, login_context, run_with_context, save_context,
    switch_context,
};
pub use config::{ContextMetadata, get_current_tracking, list_contexts, validate_context_name};
