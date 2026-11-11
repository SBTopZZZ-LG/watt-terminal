# Watt shim. Login shells only. Same pattern as .zshenv.
_watt_shim=$ZDOTDIR
ZDOTDIR=${WATT_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zprofile ]]; then source "$ZDOTDIR/.zprofile"; fi
ZDOTDIR=$_watt_shim
