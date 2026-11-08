# AgentTerm shim. Read by every zsh started with this ZDOTDIR. Sources the user's own .zshenv
# from the real ZDOTDIR (AGENTTERM_REAL_ZDOTDIR, or $HOME when empty), then restores ZDOTDIR.
_agentterm_shim=$ZDOTDIR
ZDOTDIR=${AGENTTERM_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zshenv ]]; then source "$ZDOTDIR/.zshenv"; fi
ZDOTDIR=$_agentterm_shim
