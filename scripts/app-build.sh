#!/usr/bin/env bash
# Build Watt as a macOS app bundle (and optionally a DMG) for this Mac.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/app-build.sh [options]

Builds Watt.app for the host architecture, checks that it is validly signed
(ad-hoc signs it if not), verifies the bundle and prints a summary.

Options:
  --dmg         also build a DMG from the signed app with scripts/app-dmg.sh
                (src-tauri/target/<profile>/bundle/dmg/)
  --debug       build the debug profile instead of release
  --no-checks   skip the type check and the frontend tests before the build
  --full        also run cargo clippy --all-targets and cargo test in
                src-tauri. Slow, about 5 minutes.
  -h, --help    show this help

Environment:
  SIGN_IDENTITY  codesign identity to sign with. When set, the app is always
                 signed with it (--options runtime), even if the build already
                 verifies. When unset, the app is ad-hoc signed only if the
                 build does not verify.

The last line of stdout is APP=<path to Watt.app>. scripts/app-install.sh
uses that path.
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

DMG=0
DEBUG=0
CHECKS=1
FULL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dmg) DMG=1 ;;
    --debug) DEBUG=1 ;;
    --no-checks) CHECKS=0 ;;
    --full) FULL=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

[ "$(uname -s)" = "Darwin" ] || die "this script builds the macOS app and needs macOS"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

# rustup installs cargo here; non-login shells do not have it on PATH.
export PATH="$HOME/.cargo/bin:$PATH"

info "preflight"
for tool in node npm cargo rustc codesign hdiutil; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool not found on PATH (Rust: https://rustup.rs, Node.js: https://nodejs.org)"
done
xcode-select -p >/dev/null 2>&1 || die "Xcode or the command line tools are missing (run: xcode-select --install)"

if [ ! -d node_modules ]; then
  info "node_modules is missing, running npm ci"
  npm ci
fi

PKG_VERSION="$(node -p "require('./package.json').version")"
CONF_VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
CARGO_VERSION="$(awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/"/, ""); print $3; exit}' src-tauri/Cargo.toml)"
[ -n "$CARGO_VERSION" ] || die "could not read the version from src-tauri/Cargo.toml"
if [ "$PKG_VERSION" != "$CONF_VERSION" ] || [ "$PKG_VERSION" != "$CARGO_VERSION" ]; then
  die "version mismatch: package.json $PKG_VERSION, src-tauri/tauri.conf.json $CONF_VERSION, src-tauri/Cargo.toml $CARGO_VERSION"
fi
info "version $PKG_VERSION (package.json, tauri.conf.json and Cargo.toml agree)"
info "node $(node --version), npm $(npm --version), $(rustc --version), $(cargo --version)"

if command -v git >/dev/null 2>&1 && git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  GIT_HASH="$(git -C "$ROOT" rev-parse --short HEAD)"
  info "git $GIT_HASH"
  if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    warn "the working tree has uncommitted changes, so this build is not exactly $GIT_HASH"
  fi
else
  GIT_HASH="unknown"
  warn "not a git checkout, so the build cannot be tied to a commit"
fi

if [ "$CHECKS" = 1 ]; then
  info "type check (npx tsc --noEmit)"
  npx tsc --noEmit
  info "frontend tests (npx vitest run)"
  npx vitest run
  if [ "$FULL" = 1 ]; then
    info "cargo clippy --all-targets (slow)"
    (cd src-tauri && cargo clippy --all-targets)
    info "cargo test --no-fail-fast (slow, about 5 minutes)"
    (cd src-tauri && cargo test --no-fail-fast)
  fi
else
  if [ "$FULL" = 1 ]; then
    warn "--full is ignored because --no-checks was given"
  fi
  warn "skipping the checks (--no-checks)"
fi

if [ "$DEBUG" = 1 ]; then
  PROFILE=debug
else
  PROFILE=release
fi

# Tauri's own DMG is never used: its copy of the app is not validly signed.
# With --dmg, scripts/app-dmg.sh makes the DMG from the signed app below.
info "building Watt ($PROFILE profile, host architecture $(uname -m), bundle: app)"
if [ "$DEBUG" = 1 ]; then
  npx tauri build --bundles app --debug
else
  npx tauri build --bundles app
fi

BUNDLE_DIR="$ROOT/src-tauri/target/$PROFILE/bundle"
APP="$BUNDLE_DIR/macos/Watt.app"
[ -d "$APP" ] || die "expected app bundle not found: $APP"

info "signing"
if [ -n "${SIGN_IDENTITY:-}" ]; then
  info "signing with SIGN_IDENTITY (hardened runtime)"
  codesign --force --deep --options runtime --sign "$SIGN_IDENTITY" "$APP" || die "signing with SIGN_IDENTITY failed"
elif SIGN_CHECK="$(codesign --verify --deep --strict "$APP" 2>&1)"; then
  info "the build is already validly signed, leaving it as is"
else
  warn "codesign verification failed: $SIGN_CHECK"
  info "ad-hoc signing (set SIGN_IDENTITY to use a real identity)"
  codesign --force --deep --sign - "$APP"
fi
codesign --verify --deep --strict "$APP" || die "the app does not verify after signing"

SIG_INFO="$(codesign -dvv "$APP" 2>&1)"
if grep -q '^Signature=adhoc' <<<"$SIG_INFO"; then
  SIGNING="ad-hoc"
else
  SIGNING="identity: $(sed -n 's/^Authority=//p' <<<"$SIG_INFO" | head -1)"
fi

info "verifying the bundle"
PLIST="$APP/Contents/Info.plist"
[ -f "$PLIST" ] || die "missing $PLIST"
BUNDLE_ID="$(plutil -extract CFBundleIdentifier raw -o - "$PLIST")"
SHORT_VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$PLIST")"
EXEC_NAME="$(plutil -extract CFBundleExecutable raw -o - "$PLIST")"
ICON_NAME="$(plutil -extract CFBundleIconFile raw -o - "$PLIST" 2>/dev/null || true)"

[ "$BUNDLE_ID" = "dev.watt.terminal" ] || die "CFBundleIdentifier is '$BUNDLE_ID', expected dev.watt.terminal"
[ "$SHORT_VERSION" = "$PKG_VERSION" ] || die "CFBundleShortVersionString is '$SHORT_VERSION', expected $PKG_VERSION"

EXEC_PATH="$APP/Contents/MacOS/$EXEC_NAME"
[ -f "$EXEC_PATH" ] || die "executable missing: $EXEC_PATH"
LIPO_INFO="$(lipo -info "$EXEC_PATH")"
HOST_ARCH="$(uname -m)"
case "$LIPO_INFO" in
  *"$HOST_ARCH"*) ;;
  *) die "executable is not built for $HOST_ARCH: $LIPO_INFO" ;;
esac

[ -n "$ICON_NAME" ] || die "Info.plist has no CFBundleIconFile"
[ -f "$APP/Contents/Resources/$ICON_NAME" ] || die "icon missing: Contents/Resources/$ICON_NAME"

APP_SIZE="$(du -sh "$APP" | cut -f1)"

DMG_PATH=""
if [ "$DMG" = 1 ]; then
  case "$(uname -m)" in
    arm64) DMG_ARCH=aarch64 ;;
    x86_64) DMG_ARCH=x86_64 ;;
    *) die "unsupported architecture for the DMG name: $(uname -m)" ;;
  esac
  DMG_DIR="$BUNDLE_DIR/dmg"
  DMG_PATH="$DMG_DIR/Watt_${PKG_VERSION}_${DMG_ARCH}.dmg"
  rm -f "$DMG_DIR/Watt_${PKG_VERSION}_"*.dmg
  mkdir -p "$DMG_DIR"
  info "building the DMG from the signed app"
  bash "$SCRIPT_DIR/app-dmg.sh" --app "$APP" --out "$DMG_PATH"
  [ -f "$DMG_PATH" ] || die "expected DMG not found: $DMG_PATH"
  DMG_SIZE="$(du -sh "$DMG_PATH" | cut -f1)"
fi

echo
echo "Watt $PKG_VERSION built from $GIT_HASH ($PROFILE)"
echo "  app:        $APP ($APP_SIZE)"
echo "  bundle id:  $BUNDLE_ID"
echo "  version:    $SHORT_VERSION"
echo "  arch:       $LIPO_INFO"
echo "  signing:    $SIGNING"
echo "  icon:       Contents/Resources/$ICON_NAME"
if [ -n "$DMG_PATH" ]; then
  echo "  dmg:        $DMG_PATH ($DMG_SIZE, hdiutil verify passed, app inside verified)"
fi
echo "APP=$APP"
