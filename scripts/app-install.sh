#!/usr/bin/env bash
# Install a built Watt.app into /Applications or ~/Applications.
set -euo pipefail

BUNDLE_ID="dev.watt.terminal"
APP_NAME="Watt.app"
RETAIN=2

usage() {
  cat <<'EOF'
Usage: scripts/app-install.sh [options]

Copies a Watt.app built by scripts/app-build.sh into a destination folder. The
previous copy is kept as a rollback. Your data and the Keychain are never touched.

Options:
  --app PATH     app to install (default: src-tauri/target/release/bundle/macos/Watt.app)
  --dest DIR     destination folder (default: /Applications if writable, else ~/Applications)
  --debug        use the debug build as the default source instead of the release build
  --open         launch Watt after installing (off by default)
  --force        if Watt has not quit after 10 seconds, kill it
  --no-quit      do not look for or quit a running Watt (for tests)
  --dry-run      print what would happen and change nothing
  --uninstall    remove the installed Watt.app from the destination, after quitting it
  -h, --help     show this help

Rollback: the previous copy is kept in ~/Library/Caches/watt-terminal/previous/
(the two newest are kept). Copy it back over the installed app to roll back.
Uninstall never removes ~/Library/Application Support/dev.watt.terminal or Keychain items.
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

warn() {
  echo "warning: $*" >&2
}

info() {
  echo "==> $*"
}

SRC=""
DEST=""
DEBUG=0
OPEN=0
FORCE=0
NO_QUIT=0
DRY_RUN=0
UNINSTALL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      [ $# -ge 2 ] || die "--app needs a path"
      SRC="$2"
      shift
      ;;
    --dest)
      [ $# -ge 2 ] || die "--dest needs a folder"
      DEST="$2"
      shift
      ;;
    --debug) DEBUG=1 ;;
    --open) OPEN=1 ;;
    --force) FORCE=1 ;;
    --no-quit) NO_QUIT=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

[ "$(uname -s)" = "Darwin" ] || die "this script installs the macOS app and needs macOS"
for tool in codesign plutil ditto xattr lsappinfo osascript; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool not found on PATH"
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT_PATH="$SCRIPT_DIR/app-install.sh"

if [ "$DEBUG" = 1 ]; then
  PROFILE=debug
else
  PROFILE=release
fi
if [ -z "$SRC" ]; then
  SRC="$ROOT/src-tauri/target/$PROFILE/bundle/macos/$APP_NAME"
fi

if [ "$NO_QUIT" = 1 ] && [ "$FORCE" = 1 ]; then
  warn "--force is ignored because --no-quit was given"
fi

plist_value() {
  plutil -extract "$2" raw -o - "$1" 2>/dev/null || true
}

is_running() {
  [ -n "$(lsappinfo find "bundleid=$BUNDLE_ID" 2>/dev/null || true)" ]
}

sudo_hint() {
  echo "error: $DEST is not writable by this user. Nothing was changed." >&2
  echo "Run the same command with sudo (this script does not run sudo itself):" >&2
  if [ "$UNINSTALL" = 1 ]; then
    printf '  sudo bash %q --uninstall --dest %q\n' "$SCRIPT_PATH" "$DEST" >&2
  else
    printf '  sudo bash %q --app %q --dest %q\n' "$SCRIPT_PATH" "$SRC" "$DEST" >&2
  fi
  echo "Or install into your own folder instead:" >&2
  printf '  bash %q --dest %q\n' "$SCRIPT_PATH" "$HOME/Applications" >&2
  exit 1
}

check_dest_writable() {
  if [ -d "$DEST" ]; then
    if [ ! -w "$DEST" ]; then
      sudo_hint
    fi
  else
    parent="$(dirname "$DEST")"
    if [ ! -d "$parent" ] || [ ! -w "$parent" ]; then
      sudo_hint
    fi
  fi
}

validate_source() {
  SRC="${SRC%/}"
  [ -d "$SRC" ] || die "source app not found: $SRC (run scripts/app-build.sh first, or pass --app)"
  case "$SRC" in
    *.app) ;;
    *) die "source is not an .app bundle: $SRC" ;;
  esac
  SRC="$(cd "$SRC" && pwd)"

  PLIST="$SRC/Contents/Info.plist"
  [ -f "$PLIST" ] || die "not an app bundle (no Contents/Info.plist): $SRC"
  SRC_ID="$(plist_value "$PLIST" CFBundleIdentifier)"
  [ "$SRC_ID" = "$BUNDLE_ID" ] || die "refusing to install: CFBundleIdentifier is '$SRC_ID', expected $BUNDLE_ID"
  SRC_VERSION="$(plist_value "$PLIST" CFBundleShortVersionString)"
  SRC_EXEC="$(plist_value "$PLIST" CFBundleExecutable)"
  if [ -z "$SRC_EXEC" ] || [ ! -f "$SRC/Contents/MacOS/$SRC_EXEC" ]; then
    die "refusing to install: executable missing (Contents/MacOS/$SRC_EXEC)"
  fi
  if ! CS_OUT="$(codesign --verify --deep --strict "$SRC" 2>&1)"; then
    die "refusing to install: codesign verification failed: $CS_OUT"
  fi
  info "source: $SRC (version ${SRC_VERSION:-unknown}, $BUNDLE_ID, signature verifies)"
}

resolve_dest() {
  if [ -z "$DEST" ]; then
    if [ -w /Applications ]; then
      DEST=/Applications
      DEST_WHY="default, writable"
    else
      DEST="$HOME/Applications"
      DEST_WHY="/Applications is not writable"
    fi
  else
    DEST_WHY="--dest"
  fi
  case "$DEST" in
    /*) ;;
    *) DEST="$PWD/$DEST" ;;
  esac
  DEST="${DEST%/}"
  info "destination: $DEST ($DEST_WHY)"
}

resolve_uninstall_dest() {
  if [ -n "$DEST" ]; then
    case "$DEST" in
      /*) ;;
      *) DEST="$PWD/$DEST" ;;
    esac
    DEST="${DEST%/}"
    return 0
  fi
  found=""
  found_count=0
  for cand in /Applications "$HOME/Applications"; do
    if [ -e "$cand/$APP_NAME" ]; then
      found="$cand"
      found_count=$((found_count + 1))
    fi
  done
  if [ "$found_count" -gt 1 ]; then
    die "Watt.app is in both /Applications and ~/Applications; pass --dest to choose"
  fi
  if [ "$found_count" -eq 0 ]; then
    info "Watt.app is not installed in /Applications or ~/Applications, nothing to uninstall"
    print_data_locations
    exit 0
  fi
  DEST="$found"
  info "found the installed app in $DEST"
}

quit_watt() {
  if [ "$NO_QUIT" = 1 ]; then
    warn "--no-quit: not looking for a running Watt"
    return 0
  fi
  if ! is_running; then
    return 0
  fi
  if [ "$DRY_RUN" = 1 ]; then
    info "would quit the running Watt ($BUNDLE_ID)"
    return 0
  fi
  info "quitting the running Watt"
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || warn "Watt did not accept the quit request"
  i=0
  while is_running; do
    if [ "$i" -ge 10 ]; then
      break
    fi
    sleep 1
    i=$((i + 1))
  done
  if ! is_running; then
    info "Watt has quit"
    return 0
  fi
  if [ "$FORCE" = 1 ]; then
    warn "Watt has not quit after 10 seconds, killing it (--force)"
    pkill -f "$DEST/$APP_NAME/Contents/MacOS/" >/dev/null 2>&1 || true
    sleep 1
    if is_running; then
      die "Watt is still running after --force. Quit it by hand and run again."
    fi
    return 0
  fi
  die "Watt is still running after 10 seconds. Quit it and run again, or pass --force."
}

prune_previous() {
  list="$(ls -1t "$CACHE" 2>/dev/null | grep '^Watt-.*\.app$' || true)"
  n=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    n=$((n + 1))
    if [ "$n" -gt "$RETAIN" ]; then
      rm -rf "$CACHE/$f"
      info "removed old rollback copy $f"
    fi
  done <<EOF
$list
EOF
}

keep_previous() {
  src="$1"
  mkdir -p "$HOME/Library/Caches/watt-terminal"
  mkdir -p "$CACHE"
  chmod 700 "$HOME/Library/Caches/watt-terminal" "$CACHE"
  name="Watt-$STAMP.app"
  if [ -e "$CACHE/$name" ]; then
    name="Watt-$STAMP-$$.app"
  fi
  if mv "$src" "$CACHE/$name"; then
    info "previous version kept at $CACHE/$name"
  else
    warn "could not keep the previous copy; it is still at $src"
    return 0
  fi
  prune_previous
}

print_data_locations() {
  echo "Your data is not touched by install or uninstall:"
  echo "  ~/Library/Application Support/dev.watt.terminal   (settings, conversations, window state)"
  if [ -e "$HOME/Library/Application Support/dev.agentterm.app" ]; then
    echo "  ~/Library/Application Support/dev.agentterm.app   (old data folder, still present)"
  fi
  echo "  Keychain item, service dev.watt.terminal          (API keys)"
  echo "  Keychain item, service dev.agentterm.app          (old API keys, if present)"
}

do_install() {
  TARGET="$DEST/$APP_NAME"
  STAGE="$DEST/.Watt.app.installing"
  BACKUP="$DEST/.Watt.app.previous"
  CACHE="$HOME/Library/Caches/watt-terminal/previous"
  STAMP="$(date +%Y%m%d-%H%M%S)"
  HAD_OLD=0

  if [ -e "$TARGET" ]; then
    info "Watt is already installed at $TARGET (version $(plist_value "$TARGET/Contents/Info.plist" CFBundleShortVersionString))"
  fi

  if [ "$DRY_RUN" = 1 ]; then
    info "would copy $SRC to $STAGE and verify the copy"
    info "would move the installed copy aside and swap the new one in as $TARGET"
    info "would keep the previous copy in $CACHE (two newest kept)"
    info "would remove the quarantine attribute from the installed copy and verify it"
    if [ "$OPEN" = 1 ]; then
      info "would open $TARGET"
    fi
    info "dry run: nothing was changed"
    return 0
  fi

  [ -d "$DEST" ] || mkdir -p "$DEST"
  rm -rf "$STAGE"
  info "copying to $STAGE"
  if ! ditto "$SRC" "$STAGE"; then
    rm -rf "$STAGE"
    die "copy failed. The installed copy was not changed."
  fi
  if ! CS_OUT="$(codesign --verify --deep --strict "$STAGE" 2>&1)"; then
    rm -rf "$STAGE"
    die "the copy does not verify. The installed copy was not changed: $CS_OUT"
  fi

  if [ -e "$BACKUP" ]; then
    warn "found a leftover $BACKUP from an earlier run, keeping it as a rollback copy"
    keep_previous "$BACKUP"
  fi
  if [ -e "$TARGET" ]; then
    if ! mv "$TARGET" "$BACKUP"; then
      rm -rf "$STAGE"
      die "could not move the installed copy aside. Nothing was changed."
    fi
    HAD_OLD=1
  fi
  if ! mv "$STAGE" "$TARGET"; then
    if [ "$HAD_OLD" = 1 ]; then
      mv "$BACKUP" "$TARGET" || warn "restore failed, the previous copy is at $BACKUP"
    fi
    rm -rf "$STAGE"
    die "could not put the new copy in place. The previous copy was restored."
  fi
  if ! CS_OUT="$(codesign --verify --deep --strict "$TARGET" 2>&1)"; then
    rm -rf "$TARGET"
    if [ "$HAD_OLD" = 1 ]; then
      mv "$BACKUP" "$TARGET" || warn "restore failed, the previous copy is at $BACKUP"
    fi
    die "the installed copy does not verify, so the previous copy was restored: $CS_OUT"
  fi
  xattr -dr com.apple.quarantine "$TARGET" >/dev/null 2>&1 || true
  # ditto keeps the build's timestamps. Touch the installed copy so the rollback
  # pruning order follows install time.
  touch "$TARGET" 2>/dev/null || true

  if [ "$HAD_OLD" = 1 ]; then
    keep_previous "$BACKUP"
  fi

  INSTALLED_VERSION="$(plist_value "$TARGET/Contents/Info.plist" CFBundleShortVersionString)"
  info "installed Watt ${INSTALLED_VERSION:-unknown} ($BUNDLE_ID) at $TARGET"
  if [ "$OPEN" = 1 ]; then
    open "$TARGET"
    info "opened $TARGET"
  fi
}

do_uninstall() {
  TARGET="$DEST/$APP_NAME"
  if [ ! -e "$TARGET" ]; then
    info "nothing to uninstall: $TARGET does not exist"
    print_data_locations
    return 0
  fi
  [ -d "$TARGET" ] || die "not a directory, refusing to remove: $TARGET"
  if [ "$DRY_RUN" = 1 ]; then
    info "would quit Watt if it is running, then remove $TARGET"
    info "dry run: nothing was changed"
    print_data_locations
    return 0
  fi
  quit_watt
  rm -rf "$TARGET"
  info "removed $TARGET"
  print_data_locations
}

if [ "$UNINSTALL" = 1 ]; then
  resolve_uninstall_dest
  check_dest_writable
  do_uninstall
else
  validate_source
  resolve_dest
  check_dest_writable
  quit_watt
  do_install
fi
