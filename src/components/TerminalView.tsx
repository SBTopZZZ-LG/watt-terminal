import { useEffect, useRef } from 'react';
import { fitToContainer, gridScreen, measureGrid, mountTerminal, setCursorHidden, term } from '../lib/terminal';
import { createKeyFilter } from '../lib/keymap';
import { createOverflowGuard } from '../lib/gridFit';
import type { InputState } from '../lib/types';

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
  /** True when keystrokes go to the PTY. False only after the shell exited, when the terminal drops them. */
  keysToPty: boolean;
  /** The effective input state (alt screen and exit included). Decides which keys are rewritten. */
  inputState: InputState;
  onInput: (data: string) => void;
  onAltChange: (active: boolean) => void;
}

export function TerminalView({ keysToPty, inputState, onInput, onAltChange }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  // Latest props for the long-lived xterm listeners, which are registered once.
  const latest = useRef({ keysToPty, inputState, onInput, onAltChange });
  useEffect(() => {
    latest.current = { keysToPty, inputState, onInput, onAltChange };
  });

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    mountTerminal(host);
    // The terminal has the keyboard from the start; the agent bar takes it only on its hotkey.
    term.focus();
    latest.current.onAltChange(term.buffer.active.type === 'alternate');

    // Rewrites Option+Arrow, Cmd+Arrow, Cmd+Backspace and Shift+Enter (see keymap.ts). A mapped key goes to the PTY
    // through term.input, so it takes the same route as typed text. xterm never sees the key.
    const decide = createKeyFilter();
    term.attachCustomKeyEventHandler((ev) => {
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

    const dataSub = term.onData((data) => latest.current.onInput(data));
    const bufferSub = term.buffer.onBufferChange((buf) => latest.current.onAltChange(buf.type === 'alternate'));

    // Fits are triggered by the host and the grid resizing, by window resizes, by devicePixelRatio changes and by font
    // loads. The grid can change size with the host unchanged (DPR, font metrics), so the grid is observed too.
    // A fit is followed by the overflow self-check. The check also runs 300 ms after each trigger, for late layout.
    let disposed = false;
    let fitTimer: ReturnType<typeof setTimeout> | undefined;
    let checkTimer: ReturnType<typeof setTimeout> | undefined;
    const checkOverflow = createOverflowGuard({
      measure: measureGrid,
      refit: () => fitToContainer(),
      onGiveUp: (m) => console.warn('terminal grid still wider than its container after refits', m),
    });
    const fitAndCheck = async () => {
      await fitToContainer();
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
    const screen = gridScreen();
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
    setCursorHidden(!keysToPty);
  }, [keysToPty]);

  return <div className="term-host" ref={hostRef} />;
}
