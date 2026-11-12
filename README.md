<p align="center">
  <img src="assets/brand/watt-logo-full.png" width="220" alt="Watt logo">
</p>

# Watt Terminal

Whatever Agentic Terminal Thing

A terminal where you and an AI agent share one persistent shell session. You run commands in the terminal as usual; the agent can see the same session (commands, output, screen, cwd) and can run commands in it. Built with Tauri v2, Rust, React and TypeScript, with xterm.js for rendering.

## Slice 1 scope

- Local `zsh -l` only, started through an integration shim. The user's own dotfiles are read and never edited.
- The agent is a mock (`MockAgent`): deterministic keyword rules, no network, no API key.
- Read tools are always allowed. `run_command` needs approval each time unless the session's auto-approve toggle is on (off by default). It only runs at an idle prompt with shell integration live.
- `send_keys` is a stub that returns an error.

## Prerequisites

- macOS
- Rust (with `cargo`)
- Node.js and npm
- zsh (the default macOS shell)

## Setup and running

```sh
npm install
npm run tauri dev
```

To keep manual runs away from your real `~/.zsh_history`, use the isolated-history variant. `WATT_HISTFILE` is a dev-only setting. When it is set, the shell's `HISTFILE` is that path. When it is unset, which is the default, the shell uses your own history file.

```sh
WATT_HISTFILE=/tmp/watt-terminal-hist npm run tauri dev
```

The shell leaves a blank row before each prompt that follows a command, and a thin separator line sits in that row. `WATT_PROMPT_GAP=0` turns the blank row off, in which case the line sits on the prompt's top edge.

To try a real model instead of the mock agent, set three variables. Keep the key out of your shell history: read it from your password manager (`NAME` is the name of your entry):

```sh
WATT_LLM_URL=https://openrouter.ai/api/v1/chat/completions \
WATT_LLM_MODEL=deepseek/deepseek-v4.1-flash \
WATT_LLM_KEY="$(gopass show -o NAME)" \
npm run tauri dev
```

Optional: `WATT_LLM_HEADERS` (`Name: value` lines, or separated by `;`; `{session_id}` is replaced by the conversation id, for example `x-opencode-session: {session_id}` for OpenCode Go), `WATT_LLM_STREAM=0` (wait for each whole reply instead of streaming), and `WATT_LLM_MAX_TOKENS` (a positive number). When URL, key or model is missing, the mock agent is used. A bad value is printed to the terminal at launch and the mock agent is used. These variables are for development only; they will move to a settings file.

The older `AGENT_TERM_*` names (for example `AGENT_TERM_LLM_URL`) still work when the matching `WATT_*` name is not set.

## Input

- The terminal is the main input. Type at the prompt as you would in any terminal. Keys go to the shell, or to the program that is running (vim, ssh, and so on).
- Mapped keys: Option+Left/Right move by word, Cmd+Left/Right go to the line start/end, and Cmd+Backspace clears the line (none of these in full-screen apps). Shift+Enter sends a newline (Ctrl+J) to a running program or full-screen app such as claude; at the prompt it stays a plain Enter. Cmd+Backspace sends Ctrl-U, which also marks the typed prompt as edited, so if the agent then refuses to run a command, press Ctrl+C in the terminal.
- Cmd+F finds text in the terminal (Enter and Shift+Enter step through matches, Esc closes). Cmd+K clears the scrollback and redraws the prompt at the top (it does nothing in full-screen apps). Cmd+Plus, Cmd+Minus and Cmd+0 change the font size, which is saved between launches.
- The bar at the bottom is for the agent. Its text is always a message to the agent, including text that starts with `!`. For example, "what was the last command" or "run `pwd`". A `run` request shows an approval card before anything is typed into the shell.
- Ctrl+Shift+A focuses the agent bar. Esc in the bar returns focus to the terminal.
- While the agent runs an approved command, a green border pulses around the terminal. It fades out when the agent is idle.

## Tests

Frontend:

```sh
npm test
npm run build
```

Backend:

```sh
cd src-tauri && cargo test
```

The Rust integration tests spawn real `zsh -l` sessions through the user's own startup files (`~/.zshenv`, `~/.zprofile`, `~/.zshrc`, `~/.zlogin`). They run with a temporary `HISTFILE` so the real `~/.zsh_history` is not written. Expect them to take about a minute, since they run one at a time.

## Build and install the macOS app

Prerequisites: macOS (Apple Silicon or Intel), the Xcode command line tools (`xcode-select --install`), Rust with `cargo` (found in `~/.cargo/bin` even when your shell does not load it), and Node.js with npm. The build runs `npm ci` if `node_modules` is missing.

```sh
npm run app:build                # release build of Watt.app, after the type check and tests
npm run app:install              # copy it to /Applications (or ~/Applications)
npm run app:release -- --open    # build, install, then launch
```

Both scripts take `--help`. Useful flags:

- `app:build`: `--dmg` also makes a DMG from the signed app (by `scripts/app-dmg.sh`, not Tauri's own DMG), `--debug` builds the debug profile, `--no-checks` skips `tsc` and `vitest`, `--full` also runs `cargo clippy` and `cargo test` (about five more minutes). Set `SIGN_IDENTITY` to always sign with a named identity (hardened runtime) instead of ad-hoc.
- `app:install`: `--dest DIR`, `--open`, `--force` (kill Watt if it has not quit after 10 seconds), `--dry-run`, `--uninstall`. Install never launches Watt unless you pass `--open`.
- `app:release` passes `--dmg`, `--debug`, `--no-checks`, `--full`, `--dest`, `--open` and `--force` through.

The build lands in `src-tauri/target/release/bundle/macos/Watt.app` (`target/debug/` with `--debug`), and the DMG (`Watt_<version>_aarch64.dmg`) in `src-tauri/target/release/bundle/dmg/`. The install goes to `/Applications/Watt.app`, or to `~/Applications` when `/Applications` is not writable.

The app is ad-hoc signed (unless `SIGN_IDENTITY` is set) and not notarised. The DMG is made from that signed app, and the app inside the DMG is checked before the script finishes. That is fine on the Mac that built it. A DMG or app copied to another Mac is blocked by Gatekeeper. Right-click the app and choose Open, or run `xattr -dr com.apple.quarantine /Applications/Watt.app`.

Rollback: before each install replaces the app, the old copy is kept in `~/Library/Caches/watt-terminal/previous/` (the two newest). To roll back, quit Watt and copy one of them over the installed app.

Uninstall with `npm run app:install -- --uninstall`. It removes the app only. Your data in `~/Library/Application Support/dev.watt.terminal` and the Keychain items are never touched, by install or uninstall. Because the build is ad-hoc signed, macOS may ask again for the old Keychain item and for any permissions after an install.

The scripts are tested with fake bundles, which need no build and never launch Watt: `bash scripts/test-app-scripts.sh`.

## Known limitations

- Mock agent only. There is no LLM yet.
- Local zsh only. No SSH.
- No "integration lost" detection. An `exec zsh` or nested shell silently loses shell integration.
- TUIs (alternate-screen apps such as `vim` or `less`) are rendered, but the agent can only read the screen. It cannot type into them, because `send_keys` is a stub.
- Conversation is not persisted across app restarts.

## Notes

Dropping a file on the terminal does nothing, as it did before: the app never inserted a dropped file's path. The native file drop is off (`dragDropEnabled: false` in `src-tauri/tauri.conf.json`), so the webview receives the drag and can take it. A file dropped on the agent's input bar is attached to the message; a drop anywhere else is cancelled (see `src/lib/dropGuard.ts`).

The bundle identifier is `dev.watt.terminal` (`src-tauri/tauri.conf.json`). It names the app's data folder and the Keychain item for API keys. The app used to be called `dev.agentterm.app`. On the first launch under the new identifier, the old data is copied, not moved:

- The old folder `~/Library/Application Support/dev.agentterm.app/` (settings, UI state, window state, conversations) is copied to `~/Library/Application Support/dev.watt.terminal/`. The copy is skipped when the new folder already has content.
- The saved API keys are copied from the old Keychain item (service `dev.agentterm.app`, account `secrets`) to a new one (service `dev.watt.terminal`). macOS may ask once for permission to read the old item.

Nothing is deleted. Once you are happy with the new app, you can remove the old copies yourself:

```sh
security delete-generic-password -s dev.agentterm.app
rm -r "$HOME/Library/Application Support/dev.agentterm.app"
```

Quit the old app before the first launch under the new name. A running old instance keeps using its own folder and its own single-instance socket, so the two apps do not see each other, and changes it makes later are not copied over. The `WATT_CONFIG_DIR` and `WATT_DATA_DIR` overrides skip the folder copy.
