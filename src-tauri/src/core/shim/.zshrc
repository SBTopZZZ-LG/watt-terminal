# Watt shim. Interactive shells. Runs the user's own .zshrc from the real ZDOTDIR, then loads
# the integration, repairs HISTFILE, and prints Hello last. The user's files are never edited.
_watt_shim=$ZDOTDIR
_watt_real=${WATT_REAL_ZDOTDIR:-$HOME}
ZDOTDIR=$_watt_real
if [[ -r $ZDOTDIR/.zshrc ]]; then source "$ZDOTDIR/.zshrc"; fi
ZDOTDIR=$_watt_shim
source "$ZDOTDIR/integration.zsh"
# /etc/zshrc on macOS sets HISTFILE from ZDOTDIR while ZDOTDIR is the shim dir. Point it back at the real file.
if [[ $HISTFILE == "$_watt_shim"/* ]]; then HISTFILE=$_watt_real/.zsh_history; fi
unset _watt_shim _watt_real
# Dev only (WATT_HISTFILE): an isolated history file. Set only when the app was started with it.
if [[ -n ${WATT_HISTFILE-} ]]; then HISTFILE=$WATT_HISTFILE; fi
unset WATT_HISTFILE
# Hello must stay last: the observer treats it as "this shell is observable".
if [[ -n ${_WATT_NONCE-} ]]; then
  _watt_hello
fi
