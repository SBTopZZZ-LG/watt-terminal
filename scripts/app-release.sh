#!/usr/bin/env bash
# Build Watt.app and install it: scripts/app-build.sh, then scripts/app-install.sh.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/app-release.sh [options]

Runs scripts/app-build.sh and then scripts/app-install.sh. Stops at the first failure.

Build options (passed to app-build.sh):
  --dmg          also build a DMG
  --debug        build the debug profile (and install the debug build)
  --no-checks    skip the type check and the frontend tests
  --full         also run cargo clippy and cargo test (slow)

Install options (passed to app-install.sh):
  --dest DIR     destination folder (default: /Applications if writable, else ~/Applications)
  --open         launch Watt after installing
  --force        kill Watt if it has not quit after 10 seconds

  -h, --help     show this help

Run "scripts/app-build.sh --help" and "scripts/app-install.sh --help" for the details.
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

BUILD_ARGS=()
INSTALL_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --dmg) BUILD_ARGS+=("--dmg") ;;
    --debug)
      BUILD_ARGS+=("--debug")
      INSTALL_ARGS+=("--debug")
      ;;
    --no-checks) BUILD_ARGS+=("--no-checks") ;;
    --full) BUILD_ARGS+=("--full") ;;
    --open) INSTALL_ARGS+=("--open") ;;
    --force) INSTALL_ARGS+=("--force") ;;
    --dest)
      [ $# -ge 2 ] || die "--dest needs a folder"
      INSTALL_ARGS+=("--dest" "$2")
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

LOG="$(mktemp -t watt-app-build)"
trap 'rm -f "$LOG"' EXIT

echo "==> app-build.sh"
bash "$SCRIPT_DIR/app-build.sh" ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} | tee "$LOG"

APP="$(sed -n 's/^APP=//p' "$LOG" | tail -1)"
[ -n "$APP" ] || die "app-build.sh did not print an APP= line"

echo
echo "==> app-install.sh"
bash "$SCRIPT_DIR/app-install.sh" --app "$APP" ${INSTALL_ARGS[@]+"${INSTALL_ARGS[@]}"}
