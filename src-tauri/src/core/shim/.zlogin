# Watt shim. Last file of the login chain. Sources the user's .zlogin, then restores the
# ZDOTDIR the app was started with (unset when the app had none).
_watt_shim=$ZDOTDIR
ZDOTDIR=${WATT_REAL_ZDOTDIR:-$HOME}
if [[ -r $ZDOTDIR/.zlogin ]]; then source "$ZDOTDIR/.zlogin"; fi
if [[ -n ${WATT_REAL_ZDOTDIR-} ]]; then ZDOTDIR=$WATT_REAL_ZDOTDIR; else unset ZDOTDIR; fi
unset _watt_shim WATT_REAL_ZDOTDIR
