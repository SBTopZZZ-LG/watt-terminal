// One terminal (tab): its xterm instance, the byte path from its PTY, and its connection to the Rust session. The xterm
// instance lives as long as the tab and is never disposed, so raw PTY bytes always reach it and every render is acked.
//
// Rule: nothing is written to the terminal except bytes that came from the PTY. One local exception: Cmd+K
// (clear) writes ESC[3J to drop the scrollback.

import { Channel } from '@tauri-apps/api/core';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { api, CHAT_ID, errorText, SESSION_ID } from './api';
import { agentControlAfter, NO_CONTROL, settleControl } from './agentControl';
import type { ControlSnapshot } from './agentControl';
import { clearActions } from './clear';
import { Coalescer } from './coalescer';
import { CommandMarks } from './commandMarks';
import { createFitter } from './fitter';
import type { GridMeasure } from './gridFit';
import { inputReducer } from './inputState';
import type { InputAction } from './inputState';
import { parseShellPayload } from './osc';
import { terminalFontFamily } from './platform';
import { StripeLayer } from './stripeLayer';
import { acceptedNonces, INITIAL_TAB_STATE, isSessionEvent, reduceStatus, reduceTabEvent } from './tabState';
import type { TabState } from './tabState';
import type { InputState, SessionEvent, StartResult, Status, WireEvent } from './types';
import { defaultSizeStorage, FONT_SIZE_DEFAULT, loadFontSize } from './zoom';

/**
 * The terminal background is the CSS token `--term-bg` (App.css), so it is set in one place. This is read once, when the
 * module loads. App.tsx imports App.css before this module, so the token is in the page by then. The fallback is the same
 * value, for code that runs without a document (vitest in node). Keep the two in step.
 */
const TERM_BG_FALLBACK = '#061823';

function termBackground(): string {
  if (typeof document === 'undefined') {
    return TERM_BG_FALLBACK;
  }
  const value = getComputedStyle(document.documentElement).getPropertyValue('--term-bg').trim();
  return value || TERM_BG_FALLBACK;
}

const TERM_BG = termBackground();

const THEME = {
  background: TERM_BG,
  foreground: '#d7dae0',
  cursor: '#d7dae0',
  cursorAccent: TERM_BG,
  selectionBackground: '#3a4150',
  // xterm's own scrollbar. The colours are the --scroll-thumb / --scroll-thumb-hover / --scroll-thumb-active tokens (App.css),
  // written out here because xterm takes plain colour strings. Keep them in step.
  scrollbarSliderBackground: 'rgba(111, 211, 255, 0.28)',
  scrollbarSliderHoverBackground: 'rgba(111, 211, 255, 0.5)',
  scrollbarSliderActiveBackground: '#6fd3ff',
  black: '#1b1e24',
  red: '#e06c75',
  green: '#98c379',
  yellow: '#e5c07b',
  blue: '#61afef',
  magenta: '#c678dd',
  cyan: '#56b6c2',
  white: '#d7dae0',
  brightBlack: '#5c6370',
  brightRed: '#e06c75',
  brightGreen: '#98c379',
  brightYellow: '#e5c07b',
  brightBlue: '#61afef',
  brightMagenta: '#c678dd',
  brightCyan: '#56b6c2',
  brightWhite: '#ffffff',
};

/** Find-bar highlights. Hex only, as the addon requires. Active is the selected match, drawn darker so the text stays readable. */
export const SEARCH_DECORATIONS = {
  matchBackground: '#2c3e52',
  matchBorder: '#61afef',
  matchOverviewRuler: '#61afef',
  activeMatchBackground: '#7a5c1e',
  activeMatchBorder: '#e5c07b',
  activeMatchColorOverviewRuler: '#e5c07b',
};

export interface GridInfo {
  cols: number;
  rows: number;
  hostWidth: number;
  screenWidth: number;
  devicePixelRatio: number;
}

export interface TabDeps {
  /** Shows an error to the user. */
  onError: (e: unknown) => void;
  /** Called after a terminal event has been reduced into this tab's state. */
  onTabEvent: (tab: TabController, ev: SessionEvent) => void;
  /** An agent event on the tab channel. The backend sends none; kept so nothing is dropped silently. */
  onStrayEvent: (ev: WireEvent) => void;
  /**
   * Attaches the chat's agent channel. Resolves once it is attached. `fresh` attaches again even when an attach already
   * succeeded (after a restart of a shell); the backend attach is idempotent.
   */
  attachChat: (fresh: boolean) => Promise<void>;
}

type StartCall = (rows: number, cols: number, data: Channel<ArrayBuffer>, events: Channel<WireEvent>) => Promise<StartResult>;

/** A title change is sent to the backend after this quiet time, so a burst of changes sends its last value once. */
const TITLE_SYNC_MS = 300;

export class TabController {
  readonly id: string;
  /** The xterm instance. Created with the controller, opened once by mount(). */
  readonly term: Terminal;
  readonly search: SearchAddon;
  readonly fitToContainer: () => Promise<void>;
  private readonly fitAddon: FitAddon;
  private readonly unicode: Unicode11Addon;
  private readonly marks: CommandMarks;
  private readonly coalescer: Coalescer;
  private readonly deps: TabDeps;
  /** Counts shell sessions. A batch from an older shell is not acked, because the ack would count against the new one. */
  private epoch = 0;
  private stripeLayer: StripeLayer | undefined;
  private mounted = false;
  private cursorHidden = false;
  /** The .term-host element the terminal was opened in. */
  private hostElement: HTMLElement | undefined;
  private connected = false;
  private connection: Promise<StartResult> | null = null;
  /** Bumped on every connect and restart. Callbacks from an older channel check it and drop their input. */
  private generation = 0;
  /** The nonces this tab's OSC events may carry: its shell's, and one per live nested level. Never shared with another tab. */
  private nonces: ReadonlySet<string> = new Set();
  private state: TabState = INITIAL_TAB_STATE;
  private titleTimer: ReturnType<typeof setTimeout> | undefined;
  private readonly listeners = new Set<() => void>();

  constructor(id: string, deps: TabDeps) {
    this.id = id;
    this.deps = deps;
    // Until list_tabs says otherwise, the owner is the backend's own rule: the first terminal belongs to chat `main`.
    this.state = { ...INITIAL_TAB_STATE, owner: id === SESSION_ID ? CHAT_ID : null };
    this.term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily: terminalFontFamily(),
      fontSize: loadFontSize(defaultSizeStorage()),
      scrollback: 5000,
      // A right click opens our context menu. xterm's default on macOS would first replace the selection with the word under
      // the pointer, so the menu's Copy would lose the selection the user made.
      rightClickSelectsWord: false,
      theme: THEME,
      // OSC 8 links never navigate: activation is ignored, matching the Markdown "nothing navigates" rule.
      linkHandler: {
        activate() {},
      },
    });
    this.fitAddon = new FitAddon();
    this.search = new SearchAddon();
    this.unicode = new Unicode11Addon();
    this.coalescer = new Coalescer({
      onBatch: (batch) => {
        const at = this.epoch;
        this.term.write(batch, () => {
          if (at === this.epoch) {
            api.ack(this.id, batch.length).catch(this.deps.onError);
          }
        });
      },
    });
    this.marks = new CommandMarks(this.term, () => this.stripeLayer?.schedule());
    this.term.onTitleChange((title) => {
      this.patch({ title });
      // The backend keeps the xterm title for list_terminals. Only the xterm title is sent; the fallbacks are the UI's.
      clearTimeout(this.titleTimer);
      this.titleTimer = setTimeout(() => {
        api.setTabTitle(this.id, this.state.title?.trim() || null).catch(this.deps.onError);
      }, TITLE_SYNC_MS);
    });
    this.fitToContainer = createFitter({
      connected: () => this.connected,
      proposeSize: () => {
        const dims = this.fitAddon.proposeDimensions();
        return dims ? { rows: dims.rows, cols: dims.cols } : null;
      },
      currentSize: () => this.size(),
      resizePty: (size) => api.resize(this.id, size.rows, size.cols),
      resizeTerminal: (size) => this.term.resize(size.cols, size.rows),
      onError: (e) => this.deps.onError(`resize failed: ${errorText(e)}`),
    });
  }

  /** Listener for useSyncExternalStore. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The current state, as a snapshot for useSyncExternalStore. The same object until something changes. */
  readonly getSnapshot = (): TabState => this.state;

  /** The font size xterm is drawing with now. */
  get fontSize(): number {
    return this.term.options.fontSize ?? FONT_SIZE_DEFAULT;
  }

  /** Sets the font size and refits on the next frame, after xterm has applied the new metrics. Saving is the caller's. */
  applyFontSize(size: number): void {
    this.term.options.fontSize = size;
    requestAnimationFrame(() => void this.fitToContainer());
  }

  /** Opens the terminal in `host`. Idempotent. Call before connecting, so the size is known. */
  mount(host: HTMLElement): void {
    if (this.mounted) {
      return;
    }
    this.mounted = true;
    this.hostElement = host;
    this.term.loadAddon(this.fitAddon);
    this.term.loadAddon(this.unicode);
    this.term.loadAddon(this.search);
    // Darkblood's prompt uses U+26A1, which is double width under Unicode 11 rules.
    this.term.unicode.activeVersion = '11';
    this.term.open(host);
    this.stripeLayer = new StripeLayer(this.term, this.marks);
    this.fitAddon.fit();
    this.term.parser.registerOscHandler(133, (data) => this.onShellOsc(133, data));
    this.term.parser.registerOscHandler(633, (data) => this.onShellOsc(633, data));
  }

  /** Feeds PTY bytes into the coalescer. */
  receiveBytes(buf: ArrayBuffer): void {
    this.coalescer.push(buf);
  }

  /**
   * Starts this tab's shell on first call and returns the same promise after that. Nothing here kills the shell: the Rust
   * side does that on exit.
   */
  connect(): Promise<StartResult> {
    if (this.connection === null) {
      this.connection = this.open((rows, cols, data, events) => api.startSession(this.id, rows, cols, data, events), false).then((result) => {
        this.connected = true;
        return result;
      });
    }
    return this.connection;
  }

  /**
   * Replaces an exited shell with a new one (new nonce) and attaches the channels to it. Call only after connect(). The
   * caller clears the terminal first (resetForNewSession): bytes from the old shell must not reach it.
   */
  restart(): Promise<StartResult> {
    this.connection = this.open((rows, cols, data, events) => api.restartSession(this.id, rows, cols, data, events), true);
    return this.connection;
  }

  /**
   * Clears the terminal for a new shell. Bytes the old shell already handed to xterm are parsed first, then wiped: the reset
   * is queued behind them, and the new shell's bytes arrive only after the reset is queued (restart() is called right after
   * this). Command marks are reset in the same step, so old marks cannot pair with new records.
   */
  resetForNewSession(): void {
    this.epoch += 1;
    this.coalescer.discard();
    this.term.write('', () => {
      this.term.reset();
      this.marks.reset();
    });
  }

  /** Feeds a status read into the state: the tab's part, the nonces, and the observer marks. */
  applyStatus(s: Status): void {
    this.set(reduceStatus(this.state, s));
    this.nonces = acceptedNonces(s.nonces);
    if (s.observer === 'desynced') {
      this.marks.observerDesynced();
    }
  }

  /** Re-reads only the agent-control hold from a snapshot (after `done`). */
  applyControl(s: ControlSnapshot): void {
    this.patch({ agentControl: settleControl(this.state.agentControl, s) });
  }

  /** The hold after an agent event (agent_control, done). Terminal events go through the tab channel instead. */
  applyAgentEvent(ev: WireEvent): void {
    this.patch({ agentControl: agentControlAfter(this.state.agentControl, ev) });
  }

  dispatchInput(action: InputAction): void {
    this.patch({ input: inputReducer(this.state.input, action) });
  }

  setRestarting(restarting: boolean): void {
    this.patch({ restarting });
  }

  resetAgentControl(): void {
    this.patch({ agentControl: NO_CONTROL });
  }

  focus(): void {
    // Before mount() there is no textarea to focus; the tab is focused once it is shown.
    if (this.mounted) {
      this.term.focus();
    }
  }

  /** The chat that owns this terminal, as list_tabs last said. Set by TabManager.refresh. */
  setOwner(owner: string | null): void {
    this.patch({ owner });
  }

  /**
   * Frees the xterm instance and drops the listeners. The caller closes the session first. The host element is React's
   * and goes with its component.
   */
  dispose(): void {
    clearTimeout(this.titleTimer);
    this.coalescer.discard();
    this.listeners.clear();
    this.term.dispose();
  }

  /** Cmd+K. See clear.ts. At the prompt it also sends Ctrl-L through `sendToPty`. term.clear() is not used (see clear.ts). */
  clear(state: InputState, sendToPty: (data: string) => void): void {
    const { local3J, ctrlL } = clearActions(this.term.buffer.active.type, state);
    if (local3J) {
      this.term.write('\x1b[3J');
    }
    if (ctrlL) {
      sendToPty('\x0c');
    }
  }

  /** Dims the cursor while keys are not going to the PTY, so the idle prompt looks like the input is elsewhere. */
  setCursorHidden(hidden: boolean): void {
    if (hidden === this.cursorHidden) {
      return;
    }
    this.cursorHidden = hidden;
    this.term.options.theme = hidden ? { ...THEME, cursor: THEME.background, cursorAccent: THEME.background } : THEME;
  }

  terminalSize(): { rows: number; cols: number } {
    return this.size();
  }

  /** xterm's grid element (.xterm-screen), or null before the terminal is open. */
  gridScreen(): HTMLElement | null {
    return this.term.element?.querySelector<HTMLElement>('.xterm-screen') ?? null;
  }

  /** The numbers the self-check in gridFit.ts compares. Null before the terminal is open. */
  measureGrid(): GridMeasure | null {
    const screen = this.gridScreen();
    if (!this.hostElement || !this.term.element || !screen) {
      return null;
    }
    return {
      screenRight: screen.getBoundingClientRect().right,
      hostRight: this.hostElement.getBoundingClientRect().right,
      padRight: parseFloat(getComputedStyle(this.term.element).paddingRight) || 0,
    };
  }

  /** What the grid chip shows. Widths are CSS px, measured when this is called. */
  gridInfo(): GridInfo | null {
    const screen = this.gridScreen();
    if (!this.hostElement || !screen) {
      return null;
    }
    return {
      cols: this.term.cols,
      rows: this.term.rows,
      hostWidth: Math.round(this.hostElement.getBoundingClientRect().width),
      screenWidth: Math.round(screen.getBoundingClientRect().width),
      devicePixelRatio: window.devicePixelRatio,
    };
  }

  private size(): { rows: number; cols: number } {
    return { rows: this.term.rows, cols: this.term.cols };
  }

  private open(call: StartCall, freshChat: boolean): Promise<StartResult> {
    this.generation += 1;
    const gen = this.generation;
    const { rows, cols } = this.size();
    const data = new Channel<ArrayBuffer>((buf) => {
      if (gen === this.generation) {
        this.receiveBytes(buf);
      }
    });
    const events = new Channel<WireEvent>((ev) => {
      if (gen === this.generation) {
        this.onEvent(ev);
      }
    });
    return call(rows, cols, data, events)
      .then((result) => this.deps.attachChat(freshChat).then(() => result))
      .then((result) => {
        if (gen === this.generation) {
          // A new shell has only its own nonce. The status read after the connect adds any nested levels still alive.
          this.nonces = acceptedNonces([result.nonce]);
        }
        return result;
      });
  }

  private onEvent(ev: WireEvent): void {
    if (isSessionEvent(ev)) {
      this.onTabEvent(ev);
    } else {
      this.deps.onStrayEvent(ev);
    }
  }

  private onTabEvent(ev: SessionEvent): void {
    switch (ev.type) {
      case 'levels':
        // Rust pushed, popped or pending-ed a level. The nonces are taken first, so the bytes after this event are checked
        // against them.
        this.nonces = acceptedNonces(ev.nonces);
        break;
      case 'observer':
        if (ev.status === 'desynced') {
          this.marks.observerDesynced();
        }
        break;
      case 'record_started':
        this.marks.recordStarted(ev.source.kind === 'agent');
        break;
      default:
        break;
    }
    this.set(reduceTabEvent(this.state, ev));
    this.deps.onTabEvent(this, ev);
  }

  /** Consumes OSC 133/633 so they never reach the screen. Only payloads with this tab's nonce act on the marks. */
  private onShellOsc(ident: 133 | 633, data: string): boolean {
    const ev = parseShellPayload(ident, data, this.nonces);
    if (ev === null) {
      return true;
    }
    switch (ev.kind) {
      case 'prompt_start':
        this.marks.promptStart();
        break;
      case 'command_start':
        this.marks.commandStart();
        break;
      case 'command_end':
        this.marks.commandEnd(ev.exit);
        break;
      case 'continuation':
      case 'hello':
        break;
    }
    return true;
  }

  private patch(p: Partial<TabState>): void {
    const keys = Object.keys(p) as (keyof TabState)[];
    if (keys.every((k) => p[k] === this.state[k])) {
      return;
    }
    this.set({ ...this.state, ...p });
  }

  private set(next: TabState): void {
    if (next === this.state) {
      return;
    }
    this.state = next;
    for (const listener of [...this.listeners]) {
      listener();
    }
  }
}
