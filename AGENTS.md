# gcpx — guide for scripts and AI agents

gcpx manages several Google Cloud accounts as named **contexts** (gcloud
configuration + Application Default Credentials + optional kubectl context).
This guide is also printed by `gcpx agents`.

## Rules of thumb

1. **Run commands through `gcpx run <context> -- <command>`.** It gives the
   command the context's credentials and exits with the command's own exit
   code. Don't rely on `gcpx use`: it's a shell function that only affects an
   interactive shell, and each of your commands usually runs in a new shell.
2. **Check sessions first with `gcpx status --json`.** Google expires sessions
   (16 hours by default). Exit code `3` means a context needs re-authentication.
3. **Never read credential files** (`~/.config/gcpx/*/adc.json`,
   `~/.config/gcloud/`). gcpx never prints tokens; you don't need them.
4. **Pass `--yes`** when you want the safe default instead of a confirmation
   prompt. Without a terminal and without `--yes`, gcpx fails with a message
   naming the flag to use; it never hangs waiting for input.
5. **Use `--json`** for anything you need to parse. Human-readable output may
   change; the JSON fields below are stable.

## Discover

```bash
gcpx list --json      # contexts with account, project, gcloud config
gcpx current --json   # {"context": "work", "source": "shell"|"global"} or nulls
gcpx status --json    # session health per context (exit 3 if reauth needed)
```

`gcpx list --json`:

```json
{"contexts": [{"name": "work", "account": "me@example.com", "project": "my-proj",
  "gcloud_config": "work", "kubectl_context": null, "current": true, "default": true}]}
```

`gcpx status --json`:

```json
{
  "contexts": [{
    "name": "work",
    "account": "me@example.com",
    "gcloud_config": "work",
    "config_account": "me@example.com",
    "gcloud": {"state": "reauth_required", "detail": null},
    "adc": {"state": "ok", "detail": null, "account": "me@example.com", "fresher_available": false},
    "issues": ["gcloud_reauth_required"],
    "fix": ["gcpx reauth work"]
  }],
  "needs_reauth": ["work"],
  "adc_stale": []
}
```

States: `ok`, `reauth_required`, `revoked` (needs a full `gcpx login`),
`missing`, `not_checked` (e.g. service-account ADC), `unknown` (see `detail`).
Issues: `gcloud_reauth_required`, `config_account_mismatch`, `adc_stale`,
`adc_account_mismatch`. `fix` lists the commands that resolve them.

ADC is only needed by tools that use Application Default Credentials
(Terraform, Google client libraries). If you only run `gcloud`/`kubectl`,
ignore `adc_stale`.

## Run

```bash
gcpx run work -- gcloud projects list --format=json
gcpx run work -- kubectl get pods -n default
gcpx run work -- terraform plan
```

The command receives `GOOGLE_APPLICATION_CREDENTIALS`,
`CLOUDSDK_ACTIVE_CONFIG_NAME`, `KUBECONFIG` (when the context has one) and
`GCPX_CONTEXT`. gcpx itself prints nothing unless you pass `-v`.

## Re-authenticate (the user must sign in)

You can't sign in on the user's behalf. Use the two-step flow (macOS/Linux):

```bash
gcpx reauth work --start --json
# {"context": "work", "account": "me@example.com",
#  "url": "https://accounts.google.com/o/oauth2/auth?...",
#  "expires_in": 600, "next": "gcpx reauth work --code <CODE>"}
```

1. Show the user the `url` and ask them to open it, sign in as `account`, and
   paste back the verification code Google displays.
2. Complete it (prefer stdin so the code stays out of the process list):

```bash
echo "<CODE>" | gcpx reauth work --code - --json
# {"context": "work", "account": "me@example.com", "result": "refreshed", "adc": false}
```

- Add `--adc` to `--start` if the task needs ADC (Terraform, client libraries).
- The started sign-in waits 10 minutes. `gcpx reauth work --cancel` discards it.
- The code is single-use and only works with the sign-in gcpx started on this
  machine, so it's safe for the user to paste it to you.
- If the context's gcloud configuration points at another account, `--start`
  needs `--yes` to point it back to the context's saved account.

If you run on the user's own machine with a browser and can wait, plain
`gcpx reauth work` also works: gcloud opens the browser and returns when the
user finishes.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | error (message on stderr) |
| 2 | usage error (bad arguments), or `gcpx` with no command and no terminal |
| 3 | `gcpx status`: at least one gcloud session needs re-authentication |
| n | `gcpx run`: the wrapped command's own exit code (128+signal if killed) |

## Environment

| Variable | Effect |
|---|---|
| `GCPX_NO_AUTH_CHECK=1` | skip the session check `gcpx use` / `gcpx switch` do |

## Don't

- Don't run `gcpx switch`: it changes gcloud/ADC/kubectl globally for every
  terminal of the user. Use `gcpx run`.
- Don't run `gcpx login`, `gcpx delete` or `gcpx default` unless the user asked.
- Don't pass `--force` to `reauth` unless the user asked for a fresh sign-in.
