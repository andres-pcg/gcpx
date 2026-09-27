---
name: gcpx
description: Use gcpx to work with Google Cloud (gcloud, kubectl, Terraform, client libraries) when the user has several GCP accounts or contexts, and to re-authenticate expired Google Cloud sessions. Use when a task needs a specific GCP account/project, when gcloud reports "Reauthentication required"/invalid_rapt, or when the user mentions gcpx.
---

# gcpx

gcpx keeps one **context** per Google Cloud account (gcloud configuration +
Application Default Credentials + optional kubectl context).

Run `gcpx agents` for the full guide for this installed version (JSON schemas,
exit codes, two-step sign-in). Essentials:

1. **Find the context:** `gcpx list --json` (or the repo's `.gcpx.toml`,
   `context = "<name>"`). Ask the user if it's ambiguous.
2. **Check it:** `gcpx status <context> --json`. Exit code `3` = needs
   re-authentication. Ignore `adc_stale` unless the task uses Terraform or
   Google client libraries.
3. **Run through it:** `gcpx run <context> -- <command>` (e.g.
   `gcpx run work -- gcloud projects list --format=json`). Don't use
   `gcpx use` or `gcpx switch`.
4. **Re-authenticate when needed** (the user signs in, you never do):
   - `gcpx reauth <context> --start --json` (add `--adc` if ADC is needed) →
     give the user the `url`, ask them to sign in as `account` and paste the
     verification code.
   - `echo "<CODE>" | gcpx reauth <context> --code - --json`.
5. Never read files under `~/.config/gcpx` or `~/.config/gcloud`, and never
   print tokens.
