# Watt shim. Read by every zsh started with this ZDOTDIR. Sources the user's own .zshenv
# from the real ZDOTDIR (WATT_REAL_ZDOTDIR, or $HOME when empty), then restores ZDOTDIR.
_watt_shim=$ZDOTDIR
ZDOTDIR=${WATT_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zshenv ]]; then source "$ZDOTDIR/.zshenv"; fi
ZDOTDIR=$_watt_shim
