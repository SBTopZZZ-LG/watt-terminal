// The single xterm instance. It lives for the page's lifetime: created once, opened once, never disposed.
// That keeps the byte path simple. Raw PTY bytes always reach this terminal, and every render is acked.
//
// Rule: nothing is written to the terminal except bytes that came from the PTY. One deliberate exception: Cmd+K
// (clearTerminal) writes ESC[3J locally to drop the scrollback. That only changes xterm's view, the shell is not told and
// its state is untouched. The Ctrl-L that redraws the prompt does go to the PTY, like any typed key.

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { api, errorText } from './api';
import { Coalescer } from './coalescer';
import { CommandMarks } from './commandMarks';
import { StripeLayer } from './stripeLayer';
import { getNonce, reportError } from './session';
import { parseShellPayload } from './osc';
import { createFitter } from './fitter';
import type { GridMeasure } from './gridFit';
import { clearActions } from './clear';
import { defaultSizeStorage, FONT_SIZE_DEFAULT, loadFontSize, saveFontSize, stepFontSize } from './zoom';
import type { InputState } from './types';

const THEME = {
  background: '#111318',
  foreground: '#d7dae0',
  cursor: '#d7dae0',
  cursorAccent: '#111318',
  selectionBackground: '#3a4150',
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

export const term = new Terminal({
  allowProposedApi: true,
  cursorBlink: true,
  fontFamily: 'Menlo, Monaco, "SF Mono", ui-monospace, monospace',
  fontSize: loadFontSize(defaultSizeStorage()),
  scrollback: 5000,
  theme: THEME,
});

export const fit = new FitAddon();
export const search = new SearchAddon();
const unicode = new Unicode11Addon();

/** Counts shell sessions. A batch from an older shell is not acked, because the ack would count against the new one. */
let epoch = 0;

/** Batches PTY bytes for xterm. Each batch is acked by its byte count once xterm has parsed it. */
const coalescer = new Coalescer({
  onBatch(batch) {
    const at = epoch;
    term.write(batch, () => {
      if (at === epoch) {
        api.ack(batch.length).catch(reportError);
      }
    });
  },
});

/** Created by mountTerminal, once xterm has a DOM element to draw into. */
let stripeLayer: StripeLayer | undefined;

/** Shell marks for the gutter stripes. Rust decides everything that matters; these are positional only. */
export const marks = new CommandMarks(term, () => stripeLayer?.schedule());

let mounted = false;
let cursorHidden = false;
/** The .term-host element the terminal was opened in. */
let hostElement: HTMLElement | undefined;

/** Opens the terminal in `host`. Idempotent. Call before connecting, so the size is known. */
export function mountTerminal(host: HTMLElement): void {
  if (mounted) {
    return;
  }
  mounted = true;
  hostElement = host;
  term.loadAddon(fit);
  term.loadAddon(unicode);
  term.loadAddon(search);
  // Darkblood's prompt uses U+26A1, which is double width under Unicode 11 rules.
  term.unicode.activeVersion = '11';
  term.open(host);
  stripeLayer = new StripeLayer(term, marks);
  fit.fit();
  term.parser.registerOscHandler(133, (data) => onShellOsc(133, data));
  term.parser.registerOscHandler(633, (data) => onShellOsc(633, data));
}

/**
 * Consumes OSC 133/633 so they never reach the screen. Only payloads with the session's nonce act on the marks.
 * Other payloads are dropped without being logged.
 */
function onShellOsc(ident: 133 | 633, data: string): boolean {
  const ev = parseShellPayload(ident, data, getNonce());
  if (ev === null) {
    return true;
  }
  switch (ev.kind) {
    case 'prompt_start':
      marks.promptStart();
      break;
    case 'command_start':
      marks.commandStart();
      break;
    case 'command_end':
      marks.commandEnd(ev.exit);
      break;
    case 'continuation':
    case 'hello':
      break;
  }
  return true;
}

/** Feeds PTY bytes into the coalescer. Pass as the `onBytes` of connect(). */
export function receiveBytes(buf: ArrayBuffer): void {
  coalescer.push(buf);
}

/**
 * Clears the terminal for a new shell. Bytes the old shell already handed to xterm are parsed first, then wiped:
 * the reset is queued behind them, and the new shell's bytes arrive only after the reset is queued (restart() is
 * called right after this). Command marks are reset in the same step, so old marks cannot pair with new records.
 */
export function resetForNewSession(): void {
  epoch += 1;
  coalescer.discard();
  term.write('', () => {
    term.reset();
    marks.reset();
  });
}

export function terminalSize(): { rows: number; cols: number } {
  return { rows: term.rows, cols: term.cols };
}

let connected = false;

/** Called once the session has started. Until then there is no session to resize. */
export function markConnected(): void {
  connected = true;
}

/**
 * Fits the terminal to its container and tells Rust first, then resizes xterm. Rust resizes the PTY and the
 * observer together, so the order keeps the PTY and the renderer in step. Calls are serialized (see fitter.ts).
 */
export const fitToContainer = createFitter({
  connected: () => connected,
  proposeSize: () => {
    const dims = fit.proposeDimensions();
    return dims ? { rows: dims.rows, cols: dims.cols } : null;
  },
  currentSize: () => ({ rows: term.rows, cols: term.cols }),
  resizePty: (size) => api.resize(size.rows, size.cols),
  resizeTerminal: (size) => term.resize(size.cols, size.rows),
  onError: (e) => reportError(`resize failed: ${errorText(e)}`),
});

/** xterm's grid element (.xterm-screen), or null before the terminal is open. Its size follows cols x rows. */
export function gridScreen(): HTMLElement | null {
  return term.element?.querySelector<HTMLElement>('.xterm-screen') ?? null;
}

/** The numbers the self-check in gridFit.ts compares. Null before the terminal is open. */
export function measureGrid(): GridMeasure | null {
  const screen = gridScreen();
  if (!hostElement || !term.element || !screen) {
    return null;
  }
  return {
    screenRight: screen.getBoundingClientRect().right,
    hostRight: hostElement.getBoundingClientRect().right,
    padRight: parseFloat(getComputedStyle(term.element).paddingRight) || 0,
  };
}

/** What the grid chip shows. Widths are CSS px, measured when this is called. */
export interface GridInfo {
  cols: number;
  rows: number;
  hostWidth: number;
  screenWidth: number;
  devicePixelRatio: number;
}

export function gridInfo(): GridInfo | null {
  const screen = gridScreen();
  if (!hostElement || !screen) {
    return null;
  }
  return {
    cols: term.cols,
    rows: term.rows,
    hostWidth: Math.round(hostElement.getBoundingClientRect().width),
    screenWidth: Math.round(screen.getBoundingClientRect().width),
    devicePixelRatio: window.devicePixelRatio,
  };
}

/**
 * Changes the font size by one step, or back to the default, and saves it. The fit runs on the next frame, after xterm
 * has applied the new metrics, so the PTY gets the new cols and rows. Steps past the clamp do nothing.
 */
export function zoomFont(action: 'in' | 'out' | 'reset'): void {
  const current = term.options.fontSize ?? FONT_SIZE_DEFAULT;
  const next = action === 'reset' ? FONT_SIZE_DEFAULT : stepFontSize(current, action === 'in' ? 1 : -1);
  if (next === current) {
    return;
  }
  term.options.fontSize = next;
  saveFontSize(next, defaultSizeStorage());
  requestAnimationFrame(() => void fitToContainer());
}

/**
 * Cmd+K. Clears the scrollback, and the command stripes on it, with a local ESC[3J. At the prompt it also sends Ctrl-L
 * through `sendToPty`, so the shell redraws its prompt at the top. Nothing happens on the alternate screen (see clear.ts).
 * term.clear() is not used: it disposes every marker and blanks a full-screen frame.
 */
export function clearTerminal(state: InputState, sendToPty: (data: string) => void): void {
  const { local3J, ctrlL } = clearActions(term.buffer.active.type, state);
  if (local3J) {
    term.write('\x1b[3J');
  }
  if (ctrlL) {
    sendToPty('\x0c');
  }
}

/** Dims the cursor while keys are not going to the PTY, so the idle prompt looks like the input is elsewhere. */
export function setCursorHidden(hidden: boolean): void {
  if (hidden === cursorHidden) {
    return;
  }
  cursorHidden = hidden;
  term.options.theme = hidden ? { ...THEME, cursor: THEME.background, cursorAccent: THEME.background } : THEME;
}
