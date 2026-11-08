# AgentTerm shim. Last file of the login chain. Sources the user's .zlogin, then restores the
# ZDOTDIR the app was started with (unset when the app had none).
_agentterm_shim=$ZDOTDIR
ZDOTDIR=${AGENTTERM_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zlogin ]]; then source "$ZDOTDIR/.zlogin"; fi
if [[ -n ${AGENTTERM_REAL_ZDOTDIR-} ]]; then ZDOTDIR=$AGENTTERM_REAL_ZDOTDIR; else unset ZDOTDIR; fi
unset _agentterm_shim AGENTTERM_REAL_ZDOTDIR
