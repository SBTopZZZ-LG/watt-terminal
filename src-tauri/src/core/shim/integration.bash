# Watt shell integration for bash >= 4.4. Injected into a running shell (core/inject.rs), never sourced by the shim.
# The loader evals this file inside a blob, so there is no top-level return: the install runs in _at_install.
# Every OSC 133/633 event carries n=<nonce>, the same contract as integration.zsh. The nonce comes from the plain
# variable _watt_inject_nonce, is never exported, and is never unset (PS0 and PROMPT read it on every prompt).
# Hooks: PS0 emits E and C before a command, PROMPT_COMMAND (first element) emits D, then Cwd, and PROMPT_COMMAND
# (last element) puts A and B around PS1. No DEBUG trap.

# Escape like integration.zsh: backslash -> \\ ; ';' -> \x3b ; C0 bytes and DEL -> \xHH
_at_esc() {
  local LC_ALL=C s=$1 out='' i c code
  for (( i = 0; i < ${#s}; i++ )); do
    c=${s:i:1}
    case $c in
      \\) out+='\\' ;;
      \;) out+='\x3b' ;;
      *)
        printf -v code '%d' "'$c"
        if (( code < 32 || code == 127 )); then printf -v c '\\x%02x' "$code"; fi
        out+=$c ;;
    esac
  done
  REPLY=$out
}

# Newest history entry: REPLY = its number ('' if none), _at_htext = its text (leading spaces stripped)
_at_hist() {
  local h lead n t
  h=$(HISTTIMEFORMAT= history 1)
  lead=${h%%[![:space:]]*}; h=${h#"$lead"}
  n=${h%%[[:space:]]*}; t=${h#"$n"}
  lead=${t%%[![:space:]]*}; t=${t#"$lead"}
  REPLY=$n; _at_htext=$t
}

# Runs in the $(...) subshell of PS0: emit E. If the history number did not advance since the previous prompt, the
# command was not recorded (ignoreboth / HISTIGNORE / history off / HISTSIZE=0), so the text is unknown and E is empty.
_at_e() {
  _at_hist
  if [[ -z $REPLY || $REPLY == "${_at_lastnum-}" ]]; then
    printf '\033]633;E;n=%s;\033\\' "$_at_nonce"
    return 0
  fi
  _at_esc "$_at_htext"
  printf '\033]633;E;n=%s;%s\033\\' "$_at_nonce" "$REPLY"
}

# Working directory of this level, escaped like E.
_at_cwd() {
  _at_esc "$PWD"
  printf '\033]633;Cwd;n=%s;%s\033\\' "$_at_nonce" "$REPLY"
}

# First statement of PROMPT_COMMAND: capture status before anything else runs.
_at_head() {
  local st=$? ps=("${PIPESTATUS[@]}")
  if (( _at_ran )); then
    _at_ran=0
    local IFS=,
    printf '\033]133;D;n=%s;exit=%d;ps=%s\033\\' "$_at_nonce" "$st" "${ps[*]}"
  fi
  _at_hist
  _at_lastnum=$REPLY
  _at_cwd
  return "$st"
}

# Last step of PROMPT_COMMAND (after starship-like prompt rewrites). Idempotent.
_at_tail() {
  case ${PS1-} in *'${_at_pA}'*) ;; *) PS1='\[${_at_pA}\]'"${PS1-}"'\[${_at_pB}\]' ;; esac
  case ${PS2-} in *'${_at_pP}'*) ;; *) PS2='\[${_at_pP}\]'"${PS2-}" ;; esac
  return 0
}

# Hello, printed after the hooks are in place. Values lose ';' and control bytes.
_at_hello() {
  local u=${USER-} h=${HOSTNAME-} v=${BASH_VERSION-}
  u=${u//[;[:cntrl:]]/}; h=${h//[;[:cntrl:]]/}; v=${v//[;[:cntrl:]]/}
  printf '\033]633;Hello;n=%s;v=1;sh=bash;ver=%s;user=%s;host=%s;pid=%s\033\\' "$_at_nonce" "$v" "$u" "$h" "$$"
}

_at_install() {
  if [[ -n ${_at_installed-} ]]; then
    echo 'watt: already integrated'
    return 0
  fi
  if (( BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 4) )) || [[ :${SHELLOPTS-}: == *:posix:* ]]; then
    echo "watt: Unsupported shell bash $BASH_VERSION (need >= 4.4)"
    return 0
  fi
  if ! shopt -q promptvars; then
    echo 'watt: Unsupported shell (promptvars off)'
    return 0
  fi
  _at_installed=1
  _at_nonce=${_watt_inject_nonce-}
  printf -v _at_pA '\033]133;A;n=%s\033\\' "$_at_nonce"
  printf -v _at_pB '\033]133;B;n=%s\033\\' "$_at_nonce"
  printf -v _at_pP '\033]133;P;n=%s;k=s\033\\' "$_at_nonce"
  printf -v _at_pC '\033]133;C;n=%s\033\\' "$_at_nonce"
  _at_ran=0
  # $((...)) side effect is evaluated in the PARENT (parameter expansion, not $(...)); $(_at_e) is the subshell part.
  PS0='${_at_ran:$((_at_ran=1)):0}$(_at_e)$_at_pC'
  if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a'* ]]; then
    if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
      PROMPT_COMMAND=(_at_head "${PROMPT_COMMAND[@]}" _at_tail)
    else
      # bash < 5.1 runs only element 0 of an array PROMPT_COMMAND: flatten the user's array into a string
      local _at_flat
      _at_flat=$(IFS=';'; printf '%s' "${PROMPT_COMMAND[*]}")
      PROMPT_COMMAND="_at_head;${_at_flat:+$_at_flat;}_at_tail"
    fi
  else
    PROMPT_COMMAND="_at_head;${PROMPT_COMMAND:+$PROMPT_COMMAND;}_at_tail"
  fi
  _at_hello
}
_at_install
