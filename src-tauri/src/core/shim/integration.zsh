# AgentTerm shell integration. Sourced by the shim .zshrc after the user's config, or evaluated from an injected blob
# (core/inject.rs). It has no top-level return, so eval is safe.
# Every OSC 133/633 event carries n=<nonce>; the Rust scanner (core/scanner.rs) ignores any event without it.
# Event order per prompt: E, C (command) / D (exit), Cwd and OSC 7 (cwd), then titles, then A and B (prompt).

# E payload escaping: backslash, ';' and every C0 byte (including ESC and BEL) become \\ or \xHH,
# so the payload has no raw ';' or terminator bytes.
_agentterm_escape_e() {
  local s=$1 out='' c
  local -i i
  for (( i = 1; i <= ${#s}; i++ )); do
    c=${s[i]}
    case $c in
      \\) out+="\\\\" ;;
      \;) out+='\x3b' ;;
      [[:cntrl:]]) printf -v c '\\x%02x' "'${s[i]}"; out+=$c ;;
      *) out+=$c ;;
    esac
  done
  REPLY=$out
}

# Percent-encode a path for OSC 7 (byte-wise, C locale).
_agentterm_urlencode() {
  local LC_ALL=C s=$1 c out='' i
  for (( i = 1; i <= ${#s}; i++ )); do
    c=${s[i]}
    case $c in
      [a-zA-Z0-9/._~-]) out+=$c ;;
      *) printf -v c '%%%02X' "'$c"; out+=$c ;;
    esac
  done
  REPLY=$out
}

_agentterm_precmd() {
  local ret=$? pst=("${pipestatus[@]}")
  local -a fixed=()
  local had_run=$_agentterm_ran
  if (( _agentterm_ran )); then
    _agentterm_ran=0
    printf '\033]133;D;n=%s;exit=%d;ps=%s\033\\' "$_AGENTTERM_NONCE" "$ret" "${(j:,:)pst}"
  fi
  if [[ ${precmd_functions[1]} != _agentterm_precmd ]]; then
    precmd_functions=(_agentterm_precmd ${precmd_functions:#_agentterm_precmd})
    fixed+=precmd-order
  fi
  # The gap is added back below only when a command ran, so drop the previous one first.
  PROMPT=${PROMPT/"$_agentterm_pA$_agentterm_gap"/"$_agentterm_pA"}
  if [[ $PROMPT != *"$_agentterm_pA"* || $PROMPT != *"$_agentterm_pB"* ]]; then
    PROMPT=${PROMPT//"$_agentterm_pA"/}
    PROMPT=${PROMPT//"$_agentterm_pB"/}
    PROMPT=$_agentterm_pA$PROMPT$_agentterm_pB
    fixed+=ps1
  fi
  if (( had_run )) && [[ $_agentterm_gap_on != 0 ]]; then
    PROMPT=${PROMPT/"$_agentterm_pA"/"$_agentterm_pA$_agentterm_gap"}
  fi
  if [[ $PS2 != *"$_agentterm_pP"* ]]; then
    PS2=${PS2//"$_agentterm_pP"/}
    PS2=$_agentterm_pP$PS2
    fixed+=ps2
  fi
  if (( ${#fixed} )); then
    printf '\033]633;Degraded;n=%s;reason=%s\033\\' "$_AGENTTERM_NONCE" "${(j:+:)fixed}"
  fi
  _agentterm_urlencode "$PWD"
  printf '\033]7;file://%s%s\033\\' "${HOST:-localhost}" "$REPLY"
  _agentterm_escape_e "$PWD"
  printf '\033]633;Cwd;n=%s;%s\033\\' "$_AGENTTERM_NONCE" "$REPLY"
  return $ret
}

_agentterm_preexec() {
  _agentterm_ran=1
  _agentterm_escape_e "$1"
  printf '\033]633;E;n=%s;%s\033\\' "$_AGENTTERM_NONCE" "$REPLY"
  printf '\033]133;C;n=%s\033\\' "$_AGENTTERM_NONCE"
}

# Hello: the observer treats a shell that has said Hello as observable. The shim prints it last (.zshrc); an injected
# shell prints it from the install branch at the bottom of this file. Values lose ';' and control bytes.
_agentterm_hello() {
  local user=${USER-} host=${HOST-} ver=${ZSH_VERSION-}
  user=${user//[[:cntrl:];]/}
  host=${host//[[:cntrl:];]/}
  ver=${ver//[[:cntrl:];]/}
  printf '\033]633;Hello;n=%s;v=1;sh=zsh;ver=%s;user=%s;host=%s;pid=%d\033\\' "$_AGENTTERM_NONCE" "$ver" "$user" "$host" $$
}

# Installs the hooks for the nonce in $1. Called once per shell.
_agentterm_start() {
  typeset -g _AGENTTERM_NONCE=$1
  typeset -g _agentterm_ran=0
  typeset -g _agentterm_pA _agentterm_pB _agentterm_pP
  printf -v _agentterm_pA '%%{\033]133;A;n=%s\033\\%%}' "$_AGENTTERM_NONCE"
  printf -v _agentterm_pB '%%{\033]133;B;n=%s\033\\%%}' "$_AGENTTERM_NONCE"
  printf -v _agentterm_pP '%%{\033]133;P;n=%s;k=s\033\\%%}' "$_AGENTTERM_NONCE"

  # A blank row before each prompt that follows a command. It is a literal newline right after pA, so A is emitted on
  # the settled cursor row and that row stays blank. The separator line sits in it. AGENT_TERM_PROMPT_GAP=0 turns it off.
  typeset -g _agentterm_gap=$'\n'
  typeset -g _agentterm_gap_on=${AGENT_TERM_PROMPT_GAP-1}

  precmd_functions=(_agentterm_precmd ${precmd_functions:#_agentterm_precmd})
  preexec_functions+=(_agentterm_preexec)
  PROMPT=$_agentterm_pA$PROMPT$_agentterm_pB
  PS2=$_agentterm_pP$PS2
}

# Already installed: say so and leave the nonce and the hooks alone. Otherwise the nonce comes from the injected blob
# (plain variable, never exported) or from the environment, as the shim has always done.
if (( ${precmd_functions[(Ie)_agentterm_precmd]} )); then
  print -r -- 'agentterm: already integrated'
elif [[ -n ${_agentterm_inject_nonce-} ]]; then
  _agentterm_start "$_agentterm_inject_nonce"
  _agentterm_hello
elif [[ -n ${AGENTTERM_NONCE-} ]]; then
  _agentterm_start "$AGENTTERM_NONCE"
  unset AGENTTERM_NONCE
fi
