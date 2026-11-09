import { useEffect, useRef } from 'react';
import { fitToContainer, mountTerminal, setCursorHidden, term } from '../lib/terminal';
import { createKeyFilter } from '../lib/keymap';
import type { InputState } from '../lib/types';

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

    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => void fitToContainer(), 50);
    });
    observer.observe(host);

    return () => {
      dataSub.dispose();
      bufferSub.dispose();
      observer.disconnect();
      clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    setCursorHidden(!keysToPty);
  }, [keysToPty]);

  return <div className="term-host" ref={hostRef} />;
}
