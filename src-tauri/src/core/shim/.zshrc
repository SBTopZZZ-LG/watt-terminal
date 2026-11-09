# AgentTerm shim. Interactive shells. Runs the user's own .zshrc from the real ZDOTDIR, then loads
# the integration, repairs HISTFILE, and prints Hello last. The user's files are never edited.
_agentterm_shim=$ZDOTDIR
_agentterm_real=${AGENTTERM_REAL_ZDOTDIR:-$HOME}
ZDOTDIR=$_agentterm_real
if [[ -r $ZDOTDIR/.zshrc ]]; then source "$ZDOTDIR/.zshrc"; fi
ZDOTDIR=$_agentterm_shim
source "$ZDOTDIR/integration.zsh"
# /etc/zshrc on macOS sets HISTFILE from ZDOTDIR while ZDOTDIR is the shim dir. Point it back at the real file.
if [[ $HISTFILE == "$_agentterm_shim"/* ]]; then HISTFILE=$_agentterm_real/.zsh_history; fi
unset _agentterm_shim _agentterm_real
# Dev only (AGENT_TERM_HISTFILE): an isolated history file. Set only when the app was started with it.
if [[ -n ${AGENTTERM_HISTFILE-} ]]; then HISTFILE=$AGENTTERM_HISTFILE; fi
unset AGENTTERM_HISTFILE
# Hello must stay last: the observer treats it as "this shell is observable".
if [[ -n ${_AGENTTERM_NONCE-} ]]; then
  printf '\033]633;Hello;n=%s;v=1;zsh=%s;pid=%d\033\\' "$_AGENTTERM_NONCE" "$ZSH_VERSION" $$
fi
