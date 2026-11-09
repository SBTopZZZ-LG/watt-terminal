import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import './App.css';
import { api, errorText } from './lib/api';
import { connect, reportError, restart, subscribe } from './lib/session';
import type { Incoming } from './lib/session';
import { fitToContainer, markConnected, marks, receiveBytes, resetForNewSession, term, terminalSize } from './lib/terminal';
import { effectiveState, focusAfter, inputReducer, INITIAL_INPUT, routeFor } from './lib/inputState';
import type { BarMode } from './lib/inputState';
import { EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, transcriptReducer } from './lib/transcript';
import type { Cwd, InputState, IntegrationStatus, ObserverStatus, Status, WireEvent } from './lib/types';
import { TerminalView } from './components/TerminalView';
import { InputBar } from './components/InputBar';
import { Transcript } from './components/Transcript';
import { StatusChip } from './components/StatusChip';

const CTRL_C = '\u0003';
// Form feed. At the prompt the shell's clear-screen widget runs and redraws the prompt; the bar keeps its text.
const CLEAR_SCREEN = '\u000c';

export default function App() {
  const [input, dispatchInput] = useReducer(inputReducer, INITIAL_INPUT);
  const effective = effectiveState(input);
  const route = routeFor(effective);

  const [barOpen, setBarOpen] = useState(false);
  // The alternate-screen strip opens for an agent message only when the hotkey asks for it.
  const barMode: BarMode = route.bar === 'collapsed' && barOpen ? 'agent_only' : route.bar;

  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, EMPTY_TRANSCRIPT);
  const [transcriptOpen, setTranscriptOpen] = useState(true);
  const [integration, setIntegration] = useState<IntegrationStatus>('booting');
  const [observer, setObserver] = useState<ObserverStatus>('ok');
  const [cwd, setCwd] = useState<Cwd | null>(null);
  const [autoApprove, setAutoApprove] = useState(false);
  const [focusToken, setFocusToken] = useState(0);
  const [restarting, setRestarting] = useState(false);

  // Read by long-lived listeners (keystrokes, hotkey), so they always see the current route.
  const routeRef = useRef(route);
  useEffect(() => {
    routeRef.current = route;
  });
  const clientIds = useRef(0);
  const prevEffective = useRef<InputState>(effective);

  const applyStatus = useCallback((s: Status) => {
    dispatchInput({ type: 'rust_state', state: s.state });
    setIntegration(s.integration);
    setObserver(s.observer);
    setCwd(s.cwd);
    setAutoApprove(s.auto_approve);
    dispatchTranscript({ type: 'sync_busy', busy: s.agent_busy });
    if (s.observer === 'desynced') {
      marks.observerDesynced();
    }
  }, []);

  // Connect, then load the snapshot. The snapshot covers events that arrived before this listener was up.
  useEffect(() => {
    const handleEvent = (ev: WireEvent) => {
      switch (ev.type) {
        case 'state':
          dispatchInput({ type: 'rust_state', state: ev.state });
          return;
        case 'integration':
          setIntegration(ev.status);
          return;
        case 'observer':
          setObserver(ev.status);
          if (ev.status === 'desynced') {
            marks.observerDesynced();
          }
          return;
        case 'cwd':
          setCwd(ev.cwd);
          return;
        case 'record_started':
          marks.recordStarted(ev.source.kind === 'agent');
          return;
        case 'record_finished':
          return;
        default:
          dispatchTranscript({ type: 'agent_event', event: ev });
      }
    };

    const onMessage = (msg: Incoming) => {
      if (msg.kind === 'error') {
        dispatchTranscript({ type: 'notice', text: msg.message });
      } else {
        handleEvent(msg.event);
      }
    };

    const unsubscribe = subscribe(onMessage);
    const { rows, cols } = terminalSize();
    connect(rows, cols, receiveBytes)
      .then(() => {
        markConnected();
        return api.getStatus();
      })
      .then(applyStatus)
      .then(() => fitToContainer())
      .catch(reportError);
    return unsubscribe;
  }, [applyStatus]);

  // Focus follows the state: xterm when a program needs the keyboard, the bar when the prompt is back.
  useEffect(() => {
    const target = focusAfter(prevEffective.current, effective);
    prevEffective.current = effective;
    if (target === 'xterm') {
      term.focus();
    } else if (target === 'bar') {
      setFocusToken((t) => t + 1);
    }
    if (effective !== 'alt') {
      setBarOpen(false);
    }
  }, [effective]);

  // Ctrl+Shift+A opens the bar for an agent message. Capture phase, so xterm does not also get the key.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'a')) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      if (routeRef.current.bar !== 'off') {
        setBarOpen(true);
        setFocusToken((t) => t + 1);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  const handleTerminalInput = useCallback((data: string) => {
    if (routeRef.current.keys === 'pty') {
      api.writeUser(data).catch(reportError);
    }
  }, []);

  const sendRaw = (text: string) => {
    api.writeUser(text).catch(reportError);
  };

  const onShell = async (command: string): Promise<string | null> => {
    try {
      await api.submitInput(command);
      return null;
    } catch (e) {
      return errorText(e);
    }
  };

  const onAgent = async (text: string): Promise<string | null> => {
    clientIds.current += 1;
    const clientId = clientIds.current;
    // Added before the call, so the user's line lands above the agent's replies.
    dispatchTranscript({ type: 'user_sent', clientId, text });
    try {
      await api.agentMessage(text);
      return null;
    } catch (e) {
      const message = errorText(e);
      dispatchTranscript({ type: 'user_failed', clientId, message });
      return message;
    }
  };

  const answer = (callId: string, approve: boolean) => {
    dispatchTranscript({ type: 'approval_answering', callId, approve });
    api.approveTool(callId, approve).catch((e) => {
      dispatchTranscript({ type: 'approval_failed', callId, message: errorText(e) });
    });
  };

  const pendingApproval = firstPendingApproval(transcript);

  const toggleAutoApprove = () => {
    const next = !autoApprove;
    api.setAutoApprove(next).then(() => setAutoApprove(next), reportError);
  };

  const closeBar = () => {
    // Escape collapses the alternate-screen strip and gives the keyboard back to the program.
    if (route.bar === 'collapsed') {
      setBarOpen(false);
      term.focus();
    }
  };

  // Starts a new shell after the old one exited. The terminal is cleared first, so nothing from the old shell
  // shows up in the new one. Focus moves to the bar when the new shell's prompt arrives (see focusAfter).
  const restartShell = () => {
    if (restarting) {
      return;
    }
    setRestarting(true);
    resetForNewSession();
    dispatchTranscript({ type: 'session_reset' });
    const { rows, cols } = terminalSize();
    restart(rows, cols)
      .then(() => api.getStatus())
      .then(applyStatus)
      .then(() => fitToContainer())
      .catch(reportError)
      // Still true until the status is applied, so a second click cannot land while the old exit state is shown.
      .finally(() => setRestarting(false));
  };

  const approveFirst = () => {
    if (pendingApproval !== null) {
      answer(pendingApproval, true);
    }
  };

  const denyFirst = () => {
    if (pendingApproval !== null) {
      answer(pendingApproval, false);
    }
  };

  // The bar may take focus only while keys are not going to the PTY. The alt-screen hotkey is the exception: it opens
  // the bar on purpose, so while the bar is open it does not count as owning the keys.
  const keysToPty = route.keys === 'pty' && !barOpen;

  return (
    <div className="app">
      <StatusChip
        integration={integration}
        observer={observer}
        input={effective}
        cwd={cwd}
        autoApprove={autoApprove}
        onToggleAutoApprove={toggleAutoApprove}
      />
      <div className="terminal-area">
        <TerminalView
          keysToPty={route.keys === 'pty'}
          onInput={handleTerminalInput}
          onAltChange={(active) => dispatchInput({ type: 'alt', active })}
          onIdleClick={() => setFocusToken((t) => t + 1)}
        />
      </div>
      <Transcript
        items={transcript.items}
        busy={isBusy(transcript)}
        open={transcriptOpen}
        onToggle={() => setTranscriptOpen((o) => !o)}
        onApprove={answer}
      />
      <InputBar
        mode={barMode}
        hint={route.hint}
        hasPendingApproval={pendingApproval !== null}
        keysToPty={keysToPty}
        focusToken={focusToken}
        onRestart={restartShell}
        restarting={restarting}
        onShell={onShell}
        onAgent={onAgent}
        onInterrupt={() => sendRaw(CTRL_C)}
        onClearScreen={() => sendRaw(CLEAR_SCREEN)}
        onApproveFirst={approveFirst}
        onDenyFirst={denyFirst}
        onClose={closeBar}
      />
    </div>
  );
}
