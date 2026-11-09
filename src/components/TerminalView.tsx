import { useEffect, useRef } from 'react';
import { fitToContainer, mountTerminal, setCursorHidden, term } from '../lib/terminal';

interface Props {
  /** True when keystrokes go to the PTY. False at the prompt, where the input bar owns the keyboard. */
  keysToPty: boolean;
  onInput: (data: string) => void;
  onAltChange: (active: boolean) => void;
  /** A click on the terminal while the bar owns the keyboard. */
  onIdleClick: () => void;
}

export function TerminalView({ keysToPty, onInput, onAltChange, onIdleClick }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  // Latest props for the long-lived xterm listeners, which are registered once.
  const latest = useRef({ keysToPty, onInput, onAltChange, onIdleClick });
  useEffect(() => {
    latest.current = { keysToPty, onInput, onAltChange, onIdleClick };
  });

  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    mountTerminal(host);
    latest.current.onAltChange(term.buffer.active.type === 'alternate');

    const dataSub = term.onData((data) => latest.current.onInput(data));
    const bufferSub = term.buffer.onBufferChange((buf) => latest.current.onAltChange(buf.type === 'alternate'));
    const onMouseUp = () => {
      if (!latest.current.keysToPty && !term.hasSelection()) {
        latest.current.onIdleClick();
      }
    };
    host.addEventListener('mouseup', onMouseUp);

    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => void fitToContainer(), 50);
    });
    observer.observe(host);

    return () => {
      dataSub.dispose();
      bufferSub.dispose();
      host.removeEventListener('mouseup', onMouseUp);
      observer.disconnect();
      clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    setCursorHidden(!keysToPty);
  }, [keysToPty]);

  return <div className="term-host" ref={hostRef} />;
}
