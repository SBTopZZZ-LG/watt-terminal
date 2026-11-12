#!/usr/bin/env bash
# Tests for scripts/app-build.sh, app-dmg.sh, app-install.sh and app-release.sh.
# Needs no real build and never launches Watt. All paths are under a mktemp
# directory, and HOME is set to a temp home for every install call.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/watt-app-scripts-test.XXXXXX")"
cleanup() {
  chmod -R u+w "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

FAKE_HOME="$TMP/home"
DEST="$TMP/Applications"
RO_DEST="$TMP/readonly"
APPS="$TMP/apps"
CACHE="$FAKE_HOME/Library/Caches/watt-terminal/previous"
SENTINEL_DIR="$FAKE_HOME/Library/Application Support/dev.watt.terminal"
SENTINEL="$SENTINEL_DIR/sentinel.txt"
LOG="$TMP/last.log"

mkdir -p "$FAKE_HOME" "$DEST" "$APPS" "$SENTINEL_DIR"
echo "sentinel data, must never change" >"$SENTINEL"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  echo "PASS $1"
}

fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  echo "FAIL $1"
}

skip() {
  SKIP_COUNT=$((SKIP_COUNT + 1))
  echo "SKIP $1"
}

# run <ok|fail> <name> <command...>
# Runs the command with its output in $LOG. Passes when the exit status matches the expectation.
run() {
  local expect="$1" name="$2" rc
  shift 2
  if "$@" >"$LOG" 2>&1; then
    rc=0
  else
    rc=$?
  fi
  if [ "$expect" = ok ] && [ "$rc" -eq 0 ]; then
    pass "$name"
    return 0
  fi
  if [ "$expect" = fail ] && [ "$rc" -ne 0 ]; then
    pass "$name (exit $rc)"
    return 0
  fi
  fail "$name (exit $rc, expected $expect)"
  sed 's/^/    | /' "$LOG" | tail -n 20
  return 1
}

# expect_text <name> <pattern>: the last run's output contains the fixed string.
expect_text() {
  if grep -qF -- "$2" "$LOG"; then
    pass "$1"
  else
    fail "$1 (output has no '$2')"
    sed 's/^/    | /' "$LOG" | tail -n 20
  fi
}

inst() {
  env HOME="$FAKE_HOME" bash "$ROOT/scripts/app-install.sh" "$@"
}

plist_get() {
  plutil -extract "$2" raw -o - "$1" 2>/dev/null || echo ""
}

installed_version() {
  plist_get "$DEST/Watt.app/Contents/Info.plist" CFBundleShortVersionString
}

cache_versions() {
  local p out=""
  for p in "$CACHE"/Watt-*.app; do
    [ -d "$p" ] || continue
    out="$out $(plist_get "$p/Contents/Info.plist" CFBundleShortVersionString)"
  done
  echo "$out" | tr ' ' '\n' | sed '/^$/d' | sort | tr '\n' ' '
}

cache_count() {
  if [ -d "$CACHE" ]; then
    ls -1 "$CACHE" | grep -c '^Watt-.*\.app$' || true
  else
    echo 0
  fi
}

# make_app <path> <bundle id> <version> <sign: 1|0>
make_app() {
  local app="$1" bid="$2" ver="$3" sign="$4"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>watt-terminal</string>
  <key>CFBundleIconFile</key>
  <string>icon.icns</string>
  <key>CFBundleIdentifier</key>
  <string>$bid</string>
  <key>CFBundleName</key>
  <string>Watt</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$ver</string>
  <key>CFBundleVersion</key>
  <string>$ver</string>
</dict>
</plist>
PLIST
  printf '#!/bin/sh\necho fake watt\n' >"$app/Contents/MacOS/watt-terminal"
  chmod +x "$app/Contents/MacOS/watt-terminal"
  printf 'fake icon\n' >"$app/Contents/Resources/icon.icns"
  if [ "$sign" = 1 ]; then
    codesign --force --sign - "$app" >/dev/null 2>&1
  fi
}

snapshot() {
  {
    find "$DEST" -print 2>/dev/null
    find "$FAKE_HOME/Library/Caches" -print 2>/dev/null
    find "$SENTINEL_DIR" -print 2>/dev/null
  } | sort
}

echo "Testing scripts in $ROOT/scripts"
echo

# 1. Syntax, shellcheck, help
echo "-- syntax and help"
for s in app-build.sh app-dmg.sh app-install.sh app-release.sh test-app-scripts.sh; do
  run ok "bash -n $s" /bin/bash -n "$ROOT/scripts/$s" || true
done
if command -v shellcheck >/dev/null 2>&1; then
  run ok "shellcheck on app scripts" shellcheck -x "$ROOT/scripts/app-build.sh" "$ROOT/scripts/app-dmg.sh" "$ROOT/scripts/app-install.sh" "$ROOT/scripts/app-release.sh" "$ROOT/scripts/test-app-scripts.sh" || true
else
  skip "shellcheck is not installed, so the lint step did not run"
fi
for s in app-build.sh app-dmg.sh app-install.sh app-release.sh; do
  run ok "$s --help" bash "$ROOT/scripts/$s" --help || true
done
run fail "app-install.sh rejects an unknown option" bash "$ROOT/scripts/app-install.sh" --bogus || true
run fail "app-build.sh rejects an unknown option" bash "$ROOT/scripts/app-build.sh" --bogus || true
run fail "app-dmg.sh rejects an unknown option" bash "$ROOT/scripts/app-dmg.sh" --bogus || true
echo

# 2. Install round trip with fake bundles
echo "-- install round trip (fake bundles, temp HOME, --no-quit)"
make_app "$APPS/v1/Watt.app" dev.watt.terminal 0.0.1 1
make_app "$APPS/v2/Watt.app" dev.watt.terminal 0.0.2 1
make_app "$APPS/v3/Watt.app" dev.watt.terminal 0.0.3 1
make_app "$APPS/v4/Watt.app" dev.watt.terminal 0.0.4 1
make_app "$APPS/v5/Watt.app" dev.watt.terminal 0.0.5 1

run ok "fresh install of v1" inst --app "$APPS/v1/Watt.app" --dest "$DEST" --no-quit || true
if [ "$(installed_version)" = "0.0.1" ]; then pass "fresh install is version 0.0.1"; else fail "fresh install version is '$(installed_version)'"; fi
if [ "$(cache_count)" = "0" ]; then pass "fresh install keeps no rollback copy"; else fail "fresh install left rollback copies"; fi
if [ ! -e "$DEST/.Watt.app.installing" ] && [ ! -e "$DEST/.Watt.app.previous" ]; then pass "no staging or backup folder left in dest"; else fail "staging or backup folder left in dest"; fi
if [ -f "$SENTINEL" ] && grep -q "must never change" "$SENTINEL"; then pass "install leaves the data folder untouched"; else fail "install changed the data folder"; fi

sleep 1
run ok "reinstall of v2 over v1" inst --app "$APPS/v2/Watt.app" --dest "$DEST" --no-quit || true
if [ "$(installed_version)" = "0.0.2" ]; then pass "reinstall replaces the app (0.0.2)"; else fail "reinstall version is '$(installed_version)'"; fi
if [ "$(cache_versions)" = "0.0.1 " ]; then pass "previous version (0.0.1) kept as rollback"; else fail "rollback copies are '$(cache_versions)'"; fi
if [ -d "$CACHE" ]; then
  CACHE_MODE="$(stat -f '%Lp' "$CACHE")"
  if [ "$CACHE_MODE" = "700" ]; then pass "rollback folder is mode 0700"; else fail "rollback folder mode is $CACHE_MODE"; fi
fi

sleep 1
run ok "install of v3" inst --app "$APPS/v3/Watt.app" --dest "$DEST" --no-quit || true
if [ "$(cache_count)" = "2" ]; then pass "two rollback copies kept (0.0.1, 0.0.2)"; else fail "rollback count is $(cache_count)"; fi

sleep 1
run ok "install of v4 prunes the oldest rollback" inst --app "$APPS/v4/Watt.app" --dest "$DEST" --no-quit || true
if [ "$(cache_count)" = "2" ] && [ "$(cache_versions)" = "0.0.2 0.0.3 " ]; then
  pass "retention keeps the two newest rollback copies (0.0.2, 0.0.3)"
else
  fail "retention left '$(cache_versions)' (count $(cache_count))"
fi
expect_text "install logs the rollback location" "previous version kept at" || true
echo

# 3. Dry run changes nothing
echo "-- dry run"
snapshot >"$TMP/before.txt"
run ok "dry run of v5 succeeds" inst --app "$APPS/v5/Watt.app" --dest "$DEST" --no-quit --dry-run || true
expect_text "dry run says nothing was changed" "dry run: nothing was changed" || true
snapshot >"$TMP/after.txt"
if cmp -s "$TMP/before.txt" "$TMP/after.txt" && [ "$(installed_version)" = "0.0.4" ]; then
  pass "dry run leaves dest, rollback and data folders unchanged"
else
  fail "dry run changed something (see diff)"
  diff "$TMP/before.txt" "$TMP/after.txt" | sed 's/^/    | /' | head -n 20
fi
echo

# 4. Refusals
echo "-- refusals (nothing may change)"
make_app "$APPS/other/Watt.app" dev.other.app 0.0.9 1
make_app "$APPS/unsigned/Watt.app" dev.watt.terminal 0.0.9 0
make_app "$APPS/tampered/Watt.app" dev.watt.terminal 0.0.9 1
printf 'tampered\n' >>"$APPS/tampered/Watt.app/Contents/Resources/icon.icns"

snapshot >"$TMP/before.txt"
run fail "wrong bundle id is refused" inst --app "$APPS/other/Watt.app" --dest "$DEST" --no-quit || true
expect_text "wrong bundle id message" "expected dev.watt.terminal" || true
run fail "unsigned app is refused" inst --app "$APPS/unsigned/Watt.app" --dest "$DEST" --no-quit || true
expect_text "unsigned app message" "codesign verification failed" || true
run fail "tampered app is refused" inst --app "$APPS/tampered/Watt.app" --dest "$DEST" --no-quit || true
expect_text "tampered app message" "codesign verification failed" || true
run fail "missing source is refused" inst --app "$TMP/missing/Watt.app" --dest "$DEST" --no-quit || true
expect_text "missing source message" "source app not found" || true
snapshot >"$TMP/after.txt"
if cmp -s "$TMP/before.txt" "$TMP/after.txt" && [ "$(installed_version)" = "0.0.4" ]; then
  pass "refused installs leave the installed copy and rollback unchanged"
else
  fail "a refused install changed something"
  diff "$TMP/before.txt" "$TMP/after.txt" | sed 's/^/    | /' | head -n 20
fi
echo

# 5. Unwritable destination
echo "-- unwritable destination"
if [ "$(id -u)" = "0" ]; then
  skip "running as root, so a read-only folder cannot be simulated"
else
  mkdir -p "$RO_DEST"
  chmod 555 "$RO_DEST"
  run fail "unwritable dest is refused" inst --app "$APPS/v5/Watt.app" --dest "$RO_DEST" --no-quit || true
  expect_text "unwritable dest prints the sudo hint" "sudo bash" || true
  expect_text "unwritable dest hint names the dest" "--dest" || true
  chmod 755 "$RO_DEST"
  if [ ! -e "$RO_DEST/Watt.app" ]; then pass "unwritable dest was not changed"; else fail "something was written into the unwritable dest"; fi
fi
echo

# 6. Uninstall
echo "-- uninstall"
snapshot >"$TMP/before.txt"
run ok "uninstall dry run succeeds" inst --uninstall --dest "$DEST" --no-quit --dry-run || true
if [ -d "$DEST/Watt.app" ]; then pass "uninstall dry run keeps the app"; else fail "uninstall dry run removed the app"; fi
run ok "uninstall removes the installed app" inst --uninstall --dest "$DEST" --no-quit || true
expect_text "uninstall lists the data locations" "dev.watt.terminal" || true
if [ ! -e "$DEST/Watt.app" ]; then pass "installed app is gone after uninstall"; else fail "installed app still present after uninstall"; fi
if [ -f "$SENTINEL" ] && grep -q "must never change" "$SENTINEL"; then pass "uninstall leaves the data folder untouched"; else fail "uninstall changed the data folder"; fi
if [ -d "$CACHE" ] && [ "$(cache_count)" = "2" ]; then pass "uninstall leaves the rollback copies alone"; else fail "uninstall changed the rollback copies"; fi
run ok "uninstall with nothing installed succeeds" inst --uninstall --dest "$DEST" --no-quit || true
expect_text "uninstall with nothing installed says so" "nothing to uninstall" || true
echo

# 7. DMG made by app-dmg.sh from the fake signed app
echo "-- DMG (app-dmg.sh)"
DMG_DIR="$TMP/dmg"
DMG_TMP="$TMP/dmg-tmp"
DMG_OUT="$DMG_DIR/Watt_test.dmg"
DMG_MNT="$TMP/dmg-test-mount"
DMG_LOG="$TMP/dmg-check.log"
mkdir -p "$DMG_DIR" "$DMG_TMP"
make_app "$APPS/dmg/Watt.app" dev.watt.terminal 0.0.7 1

# dmg <args...>: app-dmg.sh with its work folder under $DMG_TMP, so a leftover folder is visible
dmg() {
  env TMPDIR="$DMG_TMP" bash "$ROOT/scripts/app-dmg.sh" "$@"
}

# dmg_attached: a disk image named like the test DMG is still attached
dmg_attached() {
  hdiutil info 2>/dev/null | grep -qF "Watt_test.dmg"
}

run ok "app-dmg.sh builds the DMG" dmg --app "$APPS/dmg/Watt.app" --out "$DMG_OUT" --volname WattTest || true
if [ -f "$DMG_OUT" ]; then pass "DMG file exists"; else fail "DMG file is missing"; fi
run ok "hdiutil verify passes on the DMG" hdiutil verify "$DMG_OUT" || true

# Mount the DMG here (read-only, not launched) to check what is inside it
if mkdir -p "$DMG_MNT" && hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$DMG_MNT" "$DMG_OUT" >"$DMG_LOG" 2>&1; then
  if codesign --verify --deep --strict "$DMG_MNT/Watt.app" >"$DMG_LOG" 2>&1; then
    pass "app inside the DMG verifies (codesign --verify --deep --strict)"
  else
    fail "app inside the DMG does not verify"
    sed 's/^/    | /' "$DMG_LOG" | tail -n 5
  fi
  if [ "$(plist_get "$DMG_MNT/Watt.app/Contents/Info.plist" CFBundleIdentifier)" = "dev.watt.terminal" ]; then
    pass "app inside the DMG is dev.watt.terminal"
  else
    fail "app inside the DMG has the wrong bundle id"
  fi
  if [ -L "$DMG_MNT/Applications" ] && [ "$(readlink "$DMG_MNT/Applications")" = "/Applications" ]; then
    pass "DMG has the Applications link"
  else
    fail "DMG is missing the Applications link"
  fi
  hdiutil detach "$DMG_MNT" >/dev/null 2>&1 || hdiutil detach -force "$DMG_MNT" >/dev/null 2>&1 || true
else
  fail "could not mount the DMG to inspect it"
  sed 's/^/    | /' "$DMG_LOG" | tail -n 5
fi
if dmg_attached; then fail "test mount of the DMG is still attached"; else pass "test mount of the DMG is detached"; fi

printf 'stale, not a disk image\n' >"$DMG_OUT"
run ok "rerun replaces the stale DMG" dmg --app "$APPS/dmg/Watt.app" --out "$DMG_OUT" --volname WattTest || true
run ok "hdiutil verify passes after the rerun" hdiutil verify "$DMG_OUT" || true

run fail "app-dmg.sh refuses an unsigned app" dmg --app "$APPS/unsigned/Watt.app" --out "$DMG_DIR/unsigned.dmg" || true
expect_text "unsigned app refusal message" "does not verify" || true
run fail "app-dmg.sh refuses a tampered app" dmg --app "$APPS/tampered/Watt.app" --out "$DMG_DIR/tampered.dmg" || true
expect_text "tampered app refusal message" "does not verify" || true
run fail "app-dmg.sh refuses a missing --app" dmg --out "$DMG_DIR/missing.dmg" || true
expect_text "missing --app message" "--app is required" || true
run fail "app-dmg.sh refuses a missing app path" dmg --app "$TMP/missing/Watt.app" --out "$DMG_DIR/missing.dmg" || true
expect_text "missing app path message" "app not found" || true
if [ ! -e "$DMG_DIR/unsigned.dmg" ] && [ ! -e "$DMG_DIR/tampered.dmg" ] && [ ! -e "$DMG_DIR/missing.dmg" ]; then
  pass "refused DMG builds wrote no DMG"
else
  fail "a refused DMG build wrote a DMG"
fi

if dmg_attached; then fail "a DMG build left an image attached"; else pass "no test DMG is attached after the DMG runs"; fi
if [ -z "$(ls -A "$DMG_TMP")" ]; then pass "DMG runs left no work or staging folder"; else fail "DMG runs left folders in $DMG_TMP"; ls -A "$DMG_TMP" | sed 's/^/    | /'; fi
echo

# 8. Failure mid-swap
echo "-- failure mid-swap"
skip "restore after a failure mid-swap is not simulated: the script has no fault-injection hook, and a failing mv or codesign cannot be forced from outside without changing the script"
echo

echo "Summary: $PASS_COUNT passed, $FAIL_COUNT failed, $SKIP_COUNT skipped"
if [ "$FAIL_COUNT" -gt 0 ]; then
  exit 1
fi
exit 0
