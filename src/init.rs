//! Shell integration snippets emitted by `gcpx init <shell>`.
//!
//! These wrap the `gcpx` binary in a shell function so that `gcpx use` and
//! the auto-switch hook can `eval` env-var exports into the *current shell*
//! — which a subprocess cannot do on its own.

pub fn snippet(shell: &str) -> Option<&'static str> {
    match shell {
        "zsh" => Some(ZSH),
        "bash" => Some(BASH),
        "fish" => Some(FISH),
        _ => None,
    }
}

const ZSH: &str = r#"# gcpx shell integration
gcpx() {
  case "$1" in
    use|unuse)
      local sub="$1"; shift
      local out
      out="$(command gcpx __export "$sub" --shell zsh "$@")" || return $?
      [ -n "$out" ] && eval "$out"
      ;;
    *)
      command gcpx "$@"
      ;;
  esac
}

_gcpx_auto() {
  local out
  out="$(command gcpx __export auto --shell zsh 2>/dev/null)" || return 0
  [ -n "$out" ] && eval "$out"
}
autoload -U add-zsh-hook 2>/dev/null
add-zsh-hook chpwd _gcpx_auto
_gcpx_auto
"#;

const BASH: &str = r#"# gcpx shell integration
gcpx() {
  case "$1" in
    use|unuse)
      local sub="$1"; shift
      local out
      out="$(command gcpx __export "$sub" --shell bash "$@")" || return $?
      [ -n "$out" ] && eval "$out"
      ;;
    *)
      command gcpx "$@"
      ;;
  esac
}

_gcpx_auto() {
  local out
  out="$(command gcpx __export auto --shell bash 2>/dev/null)" || return 0
  [ -n "$out" ] && eval "$out"
}
case "${PROMPT_COMMAND:-}" in
  *_gcpx_auto*) ;;
  *) PROMPT_COMMAND="_gcpx_auto;${PROMPT_COMMAND:-}" ;;
esac
_gcpx_auto
"#;

const FISH: &str = r#"# gcpx shell integration
function gcpx
  switch $argv[1]
    case use unuse
      set -l sub $argv[1]
      set -e argv[1]
      set -l out (command gcpx __export $sub --shell fish $argv); or return $status
      test -n "$out"; and eval $out
    case '*'
      command gcpx $argv
  end
end

function _gcpx_auto --on-variable PWD
  set -l out (command gcpx __export auto --shell fish 2>/dev/null); or return 0
  test -n "$out"; and eval $out
end
_gcpx_auto
"#;
