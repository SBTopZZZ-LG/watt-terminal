// The single xterm instance. It lives for the page's lifetime: created once, opened once, never disposed.
// That keeps the byte path simple. Raw PTY bytes always reach this terminal, and every render is acked.
//
// Rule: nothing is written to the terminal except bytes that came from the PTY.

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { api, errorText } from './api';
import { Coalescer } from './coalescer';
import { CommandMarks } from './commandMarks';
import { StripeLayer } from './stripeLayer';
import { getNonce, reportError } from './session';
import { parseShellPayload } from './osc';
import { createFitter } from './fitter';

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

export const term = new Terminal({
  allowProposedApi: true,
  cursorBlink: true,
  fontFamily: 'Menlo, Monaco, "SF Mono", ui-monospace, monospace',
  fontSize: 13,
  scrollback: 5000,
  theme: THEME,
});

export const fit = new FitAddon();
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

/** Opens the terminal in `host`. Idempotent. Call before connecting, so the size is known. */
export function mountTerminal(host: HTMLElement): void {
  if (mounted) {
    return;
  }
  mounted = true;
  term.loadAddon(fit);
  term.loadAddon(unicode);
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

/** Dims the cursor while keys are not going to the PTY, so the idle prompt looks like the input is elsewhere. */
export function setCursorHidden(hidden: boolean): void {
  if (hidden === cursorHidden) {
    return;
  }
  cursorHidden = hidden;
  term.options.theme = hidden ? { ...THEME, cursor: THEME.background, cursorAccent: THEME.background } : THEME;
}
