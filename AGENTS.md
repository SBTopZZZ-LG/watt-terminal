# Watt Terminal: guide for coding agents

Watt is a macOS terminal where the user and an AI agent share one persistent shell session (`zsh -l` through an integration shim). Tauri v2 + Rust backend (`src-tauri/`), React 19 + TypeScript frontend (`src/`), xterm.js for rendering. `README.md` covers usage; this file covers how to change the code safely.

## Commands

- Frontend: `npm test` (vitest), `npx tsc --noEmit`, `npm run build` (tsc + vite). Run all three before calling frontend work done.
- Backend (from `src-tauri/`, with `cargo` on PATH): `cargo check` for quick feedback, `cargo clippy` (kept warning-free), `cargo test`. The integration tests in `src-tauri/tests/` spawn real `zsh -l` sessions one at a time and take minutes; run them once at the end, not in a loop. Some are `#[ignore]`d on purpose (ssh needs Docker, one needs the real Keychain).
- Run the app: `npm run tauri dev`. Build and install the macOS app: `npm run app:release -- --open` (`scripts/`, README). `app:build` runs `tsc` and `vitest` first (`--no-checks` skips them, `--full` adds clippy and cargo test).
- `scripts/*.sh` are written for macOS's bash 3.2. `bash scripts/test-app-scripts.sh` tests them with fake bundles; it needs macOS tools (`hdiutil`, `codesign`) but never launches the app.
- Rust edition 2024. Tauri crates and the `@tauri-apps/*` packages are pinned; bump them together. No formatter or linter config exists beyond strict `tsconfig` (`noUnused*`): match the surrounding style.

## Map

- `src/App.tsx` wires the panels; `src/App.css` is the only stylesheet. `src/components/` holds React components, `src/lib/` pure logic. Put decisions (layout rules, formatting, state machines) in `src/lib/` with a colocated `*.test.ts`, and keep components thin.
- Component tests use `renderToStaticMarkup` (no DOM environment is installed), so DOM-only behaviour (focus, scrolling, observers, layout) is not covered by vitest. Test that logic as pure functions and check the rest by running the app.
- `src/lib/types.ts` mirrors the Rust serde shapes (events, loaded conversations); `src/lib/api.ts` wraps the Tauri commands. Change both sides together.
- `src-tauri/src/lib.rs`: setup, plugins, commands. `core/`: `pty.rs`, `observer.rs`, `session.rs` (the shared shell), `shim/` (shell integration scripts), `inject.rs`, `foreground.rs`, `approval.rs` (policy), `tools.rs` (tool schemas and dispatch), `chat/` (agent loop, prompt, history, context budget and trim, todo, attachments, `redact.rs`), `llm/` (provider requests, SSE), `settings/` (settings.json, models, presets, secrets), `persist/` (conversations, UI state), `migrate.rs`, `env.rs` (WATT_* names with legacy AGENT_TERM_* fallbacks).

## Invariants

- The agent bar's text is always a message to the agent, never shell input. `run_command` and `send_keys` need user approval unless a rule or auto-approve allows them (typed lines still hit the hard gate and deny rules). `run_command` runs only at an idle prompt with shell integration live. Read tools are always allowed.
- The user's dotfiles are read, never edited: the shim runs the shell with its own `ZDOTDIR` and sources the user's files. Keep test and manual runs away from real state: `WATT_HISTFILE`, `WATT_CONFIG_DIR`, `WATT_DATA_DIR` (and `WATT_SECRETS_FILE`, a 0600 file store, instead of the Keychain). Note the integration tests do execute the user's real startup files; they only redirect `HISTFILE`.
- Secrets live in the Keychain (one item holding a JSON blob), the opt-in secrets file, and process memory (plus the dev-only `WATT_LLM_KEY`). Never put them in `settings.json`, logs, fixtures or transcripts; tool output goes through `chat/redact.rs`.
- Stored conversation history is the truth. Context trimming is wire-only and never rewrites it. Token counts are estimates (chars/4, calibrated against provider usage); with no known model limit nothing is trimmed proactively.
- The bundle id is `dev.watt.terminal`. Legacy `dev.agentterm.app` data and Keychain items are copied at first launch and never deleted or overwritten (`migrate.rs`, `settings/secrets.rs`).
- CSP is strict (`tauri.conf.json`, `src/lib/csp.test.ts`): no inline scripts, no remote resources. Native file drop is off on purpose; drops are handled in the webview (`src/lib/dropGuard.ts`).

## UI and layout work

- The renderer is WKWebView (WebKit), not Chrome: use `-webkit-` prefixes where needed and check layout in WebKit as well as Chromium.
- Docks are left, right, top, bottom; a side dock can collapse to a rail; only top and bottom can auto-size (`src/lib/panelLayout.ts`: side minimum 240px, terminal minimum 320px). The window minimum is 800x600. Check narrow and short sizes, banners on and off, and the font-size steps.
- Prefer whole-pixel sizes: fractional heights on Retina round into phantom scrollbars. Keep overflow hidden until content really overflows.
- Colours come from the tokens under `[data-theme="blueprint"]` in `App.css`. Animate only `transform`, `opacity` or `background-position`, and honour `prefers-reduced-motion`.
- Look at screenshots of the extreme sizes before trusting geometry assertions.

## Working practices

- Read the neighbouring code and reuse its helpers first. Add tests next to what you change. Change an existing assertion only when the behaviour change is intended, and say which one and why.
- Verify with real command output. Report what passed, failed, was skipped and remains unverified; never claim a check passed without running it.
- Keep changes scoped: no drive-by refactors, no new dependency without a reason, no loosening a check to get green.
- Commit small, with imperative messages that explain the why. Don't commit `dist/`, `src-tauri/target/` or local task notes.
