# AgentTerm shim. Login shells only. Same pattern as .zshenv.
_agentterm_shim=$ZDOTDIR
ZDOTDIR=${AGENTTERM_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zprofile ]]; then source "$ZDOTDIR/.zprofile"; fi
ZDOTDIR=$_agentterm_shim
