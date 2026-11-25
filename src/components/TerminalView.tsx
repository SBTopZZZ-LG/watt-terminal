import { useEffect, useRef } from 'react';
import type { CSSProperties } from 'react';
import type { Terminal } from '@xterm/xterm';
import type { TabController } from '../lib/tab';
import { clipboardActionFor, createKeyFilter } from '../lib/keymap';
import { copyText, readClipboardText } from '../lib/clipboard';
import { createOverflowGuard } from '../lib/gridFit';
import { reportError } from '../lib/tabManager';
import type { InputState } from '../lib/types';

/** Copies the selection, then clears it, so a later Ctrl+C interrupts again. Nothing happens without a selection. */
function copySelection(term: Terminal): void {
  if (!term.hasSelection()) {
    return;
  }
  const text = term.getSelection();
  void copyText(text, { clipboard: navigator.clipboard, document }).then((ok) => {
    if (ok) {
      term.clearSelection();
    }
  });
}

/** Pastes the clipboard's text as typed text would be: through xterm, so bracketed paste applies. Ctrl+V and the context menu's Paste both use it. */
export async function pasteFromClipboard(term: Terminal): Promise<void> {
  const text = await readClipboardText(navigator.clipboard);
  if (text === null) {
    reportError('Could not read the clipboard.');
  } else if (text !== '') {
    term.paste(text);
  }
}

/**
 * Calls `onChange` whenever window.devicePixelRatio changes (the window moved to another display, or was zoomed).
 * A media query matches one ratio, so after each change the listener is re-registered for the new one.
 * Returns the stop function.
 */
function watchPixelRatio(onChange: () => void): () => void {
  let stopped = false;
  let query: MediaQueryList | undefined;
  const onQueryChange = () => {
    onChange();
    listen();
  };
  const listen = () => {
    if (stopped) {
      return;
    }
    query = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    query.addEventListener('change', onQueryChange, { once: true });
  };
  listen();
  return () => {
    stopped = true;
    query?.removeEventListener('change', onQueryChange);
  };
}

interface Props {
  /** The terminal this host shows. */
  controller: TabController;
  /** Only the active terminal takes keys, alt-screen changes and the cursor. The others are drawn hidden (see the return). */
  active: boolean;
  /** True when keystrokes go to the PTY. False only after the shell exited, when the terminal drops them. */
  keysToPty: boolean;
  /** The effective input state (alt screen and exit included). Decides which keys are rewritten. */
  inputState: InputState;
  onInput: (data: string) => void;
  onAltChange: (active: boolean) => void;
}

/** A hidden host keeps its size (so the fit still works) and takes no pointer or focus. */
const HIDDEN: CSSProperties = { visibility: 'hidden', pointerEvents: 'none' };

export function TerminalView({ controller, active, keysToPty, inputState, onInput, onAltChange }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  // Latest props for the long-lived xterm listeners, which are registered once.
  const latest = useRef({ active, keysToPty, inputState, onInput, onAltChange });
  useEffect(() => {
    latest.current = { active, keysToPty, inputState, onInput, onAltChange };
  });

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const term = controller.term;
    controller.mount(host);
    // The terminal has the keyboard from the start; the agent bar takes it only on its hotkey.
    if (latest.current.active) {
      term.focus();
      latest.current.onAltChange(term.buffer.active.type === 'alternate');
    }

    // Rewrites Option+Arrow, Cmd+Arrow, Cmd+Backspace and Shift+Enter (see keymap.ts). A mapped key goes to the PTY
    // through term.input, so it takes the same route as typed text. xterm never sees the key.
    const decide = createKeyFilter();
    term.attachCustomKeyEventHandler((ev) => {
      // Copy and paste off macOS (see keymap.ts). Consumed here, so xterm does not send ETX for Ctrl+C or ^V.
      const clip = clipboardActionFor(ev, term.hasSelection());
      if (clip !== null) {
        ev.preventDefault();
        if (clip === 'copy') {
          copySelection(term);
        } else {
          void pasteFromClipboard(term);
        }
        return false;
      }
      const { data, consume } = decide(ev, latest.current.inputState);
      if (data !== null) {
        term.input(data, true);
      }
      if (consume) {
        ev.preventDefault();
        return false;
      }
      return true;
    });

    const dataSub = term.onData((data) => {
      if (latest.current.active) {
        latest.current.onInput(data);
      }
    });
    const bufferSub = term.buffer.onBufferChange((buf) => {
      if (latest.current.active) {
        latest.current.onAltChange(buf.type === 'alternate');
      }
    });

    // Fits are triggered by the host and the grid resizing, by window resizes, by devicePixelRatio changes and by font
    // loads. The grid can change size with the host unchanged (DPR, font metrics), so the grid is observed too.
    // A fit is followed by the overflow self-check. The check also runs 300 ms after each trigger, for late layout.
    let disposed = false;
    let fitTimer: ReturnType<typeof setTimeout> | undefined;
    let checkTimer: ReturnType<typeof setTimeout> | undefined;
    const checkOverflow = createOverflowGuard({
      measure: () => controller.measureGrid(),
      refit: () => controller.fitToContainer(),
      onGiveUp: (m) => console.warn('terminal grid still wider than its container after refits', m),
    });
    const fitAndCheck = async () => {
      await controller.fitToContainer();
      await checkOverflow();
    };
    const trigger = (delay: number) => {
      if (disposed) {
        return;
      }
      clearTimeout(fitTimer);
      fitTimer = setTimeout(() => void fitAndCheck(), delay);
      clearTimeout(checkTimer);
      checkTimer = setTimeout(() => void checkOverflow(), 300);
    };

    const observer = new ResizeObserver(() => trigger(50));
    observer.observe(host);
    const screen = controller.gridScreen();
    if (screen) {
      observer.observe(screen);
    }

    const onWindowResize = () => trigger(50);
    window.addEventListener('resize', onWindowResize);
    const fonts = document.fonts;
    const onFonts = () => trigger(0);
    void fonts.ready.then(onFonts);
    fonts.addEventListener('loadingdone', onFonts);
    const stopPixelRatio = watchPixelRatio(() => requestAnimationFrame(() => trigger(0)));

    return () => {
      disposed = true;
      dataSub.dispose();
      bufferSub.dispose();
      observer.disconnect();
      window.removeEventListener('resize', onWindowResize);
      fonts.removeEventListener('loadingdone', onFonts);
      stopPixelRatio();
      clearTimeout(fitTimer);
      clearTimeout(checkTimer);
    };
  }, []);

  useEffect(() => {
    if (active) {
      controller.setCursorHidden(!keysToPty);
    }
  }, [active, controller, keysToPty]);

  return <div className="term-host" ref={hostRef} style={active ? undefined : HIDDEN} />;
}
