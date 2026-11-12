#!/usr/bin/env bash
# Make a DMG from a signed Watt.app: the app plus a link to /Applications.
# scripts/app-build.sh --dmg uses this. Tauri's own DMG is not used, because
# the copy of the app inside it is not validly signed.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/app-dmg.sh --app PATH --out FILE [--volname NAME]

Makes a compressed read-only DMG (UDZO) holding a copy of the app and a link
to /Applications. The app must pass codesign --verify --deep --strict first.
The DMG is checked with hdiutil verify, then mounted read-only (never launched)
and the app inside is checked the same way. An existing FILE is replaced.

Options:
  --app PATH      the Watt.app to put in the DMG
  --out FILE      the DMG to write
  --volname NAME  the volume name (default: Watt)
  -h, --help      show this help
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

usage_error() {
  echo "error: $*" >&2
  usage >&2
  exit 2
}

info() {
  echo "==> $*"
}

APP=""
OUT=""
VOLNAME="Watt"

while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      [ $# -ge 2 ] || usage_error "--app needs a path"
      APP="$2"
      shift
      ;;
    --out)
      [ $# -ge 2 ] || usage_error "--out needs a file"
      OUT="$2"
      shift
      ;;
    --volname)
      [ $# -ge 2 ] || usage_error "--volname needs a name"
      VOLNAME="$2"
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    *) usage_error "unknown option: $1" ;;
  esac
  shift
done

[ "$(uname -s)" = "Darwin" ] || die "this script makes the macOS DMG and needs macOS"
for tool in codesign hdiutil ditto; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool not found on PATH"
done

[ -n "$APP" ] || usage_error "--app is required"
[ -n "$OUT" ] || usage_error "--out is required"

APP="${APP%/}"
[ -d "$APP" ] || die "app not found: $APP"
case "$APP" in
  *.app) ;;
  *) die "not an .app bundle: $APP" ;;
esac
APP_NAME="$(basename "$APP")"

if ! CS_OUT="$(codesign --verify --deep --strict "$APP" 2>&1)"; then
  die "refusing to make a DMG: the app does not verify: $CS_OUT"
fi

OUT_DIR="$(dirname "$OUT")"
mkdir -p "$OUT_DIR"
OUT="$(cd "$OUT_DIR" && pwd)/$(basename "$OUT")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/watt-dmg.XXXXXX")"
STAGE="$WORK/stage"
MNT="$WORK/mnt"
MOUNTED=0
DETACH_FAILED=0

# Detach the image we mounted. Returns non-zero if it is still attached.
detach_mount() {
  [ "$MOUNTED" = 1 ] || return 0
  if hdiutil detach "$MNT" >/dev/null 2>&1 || hdiutil detach -force "$MNT" >/dev/null 2>&1; then
    MOUNTED=0
    return 0
  fi
  DETACH_FAILED=1
  return 1
}

cleanup() {
  local rc=$?
  if ! detach_mount; then
    echo "error: could not detach $MNT. Detach it by hand with: hdiutil detach -force \"$MNT\"" >&2
  fi
  if [ "$MOUNTED" = 0 ]; then
    rm -rf "$WORK"
  else
    echo "error: left the work folder in place because it is still mounted: $WORK" >&2
  fi
  if [ "$rc" -eq 0 ] && [ "$DETACH_FAILED" = 1 ]; then
    rc=1
  fi
  exit "$rc"
}
trap cleanup EXIT

mkdir -p "$STAGE" "$MNT"
info "staging $APP_NAME and the Applications link"
ditto "$APP" "$STAGE/$APP_NAME"
ln -s /Applications "$STAGE/Applications"

info "creating $OUT (volume $VOLNAME)"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$OUT"

info "verifying the DMG (hdiutil verify)"
hdiutil verify "$OUT"

info "checking the app inside the DMG (mounted read-only, not launched)"
if ! hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$MNT" "$OUT" >"$WORK/attach.log" 2>&1; then
  cat "$WORK/attach.log" >&2
  die "could not mount $OUT"
fi
MOUNTED=1

INNER="$MNT/$APP_NAME"
[ -d "$INNER" ] || die "$APP_NAME is not in the DMG"
if ! CS_OUT="$(codesign --verify --deep --strict "$INNER" 2>&1)"; then
  die "the app inside the DMG does not verify: $CS_OUT"
fi
[ -L "$MNT/Applications" ] || die "no Applications link in the DMG"
[ "$(readlink "$MNT/Applications")" = "/Applications" ] || die "the Applications link in the DMG does not point to /Applications"

detach_mount || die "could not detach $MNT"

info "DMG ready: $OUT (inner app verifies, Applications link present)"
