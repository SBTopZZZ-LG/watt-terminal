import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import './App.css';
import { api, errorText } from './lib/api';
import { connect, reportError, restart, subscribe } from './lib/session';
import type { Incoming } from './lib/session';
import { clearTerminal, fitToContainer, markConnected, marks, receiveBytes, resetForNewSession, term, terminalSize, zoomFont } from './lib/terminal';
import { appHotkeyFor } from './lib/appHotkeys';
import { effectiveState, focusAfter, inputReducer, INITIAL_INPUT, isFocusBarHotkey, routeFor } from './lib/inputState';
import type { BarMode } from './lib/inputState';
import { agentControlAfter, borderActive, NO_CONTROL } from './lib/agentControl';
import type { AgentControl } from './lib/agentControl';
import { EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, transcriptReducer } from './lib/transcript';
import type { Cwd, InputState, IntegrationStatus, ObserverStatus, Status, WireEvent } from './lib/types';
import { TerminalView } from './components/TerminalView';
import { InputBar } from './components/InputBar';
import { Transcript } from './components/Transcript';
import { StatusChip } from './components/StatusChip';
import { FindBar } from './components/FindBar';

const CTRL_C = '\u0003';

export default function App() {
  const [input, dispatchInput] = useReducer(inputReducer, INITIAL_INPUT);
  const effective = effectiveState(input);
  const route = routeFor(effective);

  // The bar counts as open while its input has focus, or the hotkey asked for it. Only the alternate screen cares: an
  // open bar stays as the chat there, so a program starting or ending never takes the input's focus away.
  const [barOpen, setBarOpen] = useState(false);
  const barMode: BarMode = route.bar === 'collapsed' && barOpen ? 'chat' : route.bar;

  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, EMPTY_TRANSCRIPT);
  const [transcriptOpen, setTranscriptOpen] = useState(true);
  const [integration, setIntegration] = useState<IntegrationStatus>('booting');
  const [observer, setObserver] = useState<ObserverStatus>('ok');
  const [cwd, setCwd] = useState<Cwd | null>(null);
  const [autoApprove, setAutoApprove] = useState(false);
  const [focusToken, setFocusToken] = useState(0);
  const [restarting, setRestarting] = useState(false);
  // The agent's hold on the shell: an approved call executing, or a command it started still running (see
  // agentControl.ts). Drives the green border around the terminal.
  const [agentControl, setAgentControl] = useState<AgentControl>(NO_CONTROL);
  // The find bar is mounted while open. Its token moves focus into it when Cmd+F is pressed again.
  const [findOpen, setFindOpen] = useState(false);
  const [findToken, setFindToken] = useState(0);

  // Read by long-lived listeners (keystrokes, hotkey), so they always see the current route.
  const routeRef = useRef(route);
  useEffect(() => {
    routeRef.current = route;
  });
  // Read by the Cmd+K listener, which is registered once.
  const effectiveRef = useRef(effective);
  useEffect(() => {
    effectiveRef.current = effective;
  });
  const clientIds = useRef(0);
  const prevEffective = useRef<InputState>(effective);

  const applyStatus = useCallback((s: Status) => {
    dispatchInput({ type: 'rust_state', state: s.state });
    setIntegration(s.integration);
    setObserver(s.observer);
    setCwd(s.cwd);
    setAutoApprove(s.auto_approve);
    setAgentControl((c) => ({ ...c, call: s.agent_control }));
    dispatchTranscript({ type: 'sync_busy', busy: s.agent_busy });
    if (s.observer === 'desynced') {
      marks.observerDesynced();
    }
  }, []);

  // Connect, then load the snapshot. The snapshot covers events that arrived before this listener was up.
  useEffect(() => {
    const handleEvent = (ev: WireEvent) => {
      setAgentControl((c) => agentControlAfter(c, ev));
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

  // Focus follows the shell's end, not its commands (see focusAfter): the bar takes it when the shell exits, the
  // terminal when a new one starts.
  useEffect(() => {
    const target = focusAfter(prevEffective.current, effective);
    prevEffective.current = effective;
    if (target === 'xterm') {
      term.focus();
    } else if (target === 'bar') {
      setFocusToken((t) => t + 1);
    }
  }, [effective]);

  // Ctrl+Shift+A (Cmd+Shift+A) focuses the agent bar, from anywhere it is offered. Cmd+F, Cmd+K and Cmd+=/-/0 (see
  // appHotkeys.ts) are handled here too. Capture phase, so xterm does not also get the key, and a handled key is
  // stopped here so it never reaches the PTY.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isFocusBarHotkey(e)) {
        e.preventDefault();
        e.stopPropagation();
        const { bar } = routeRef.current;
        if (bar !== 'exited') {
          if (bar === 'collapsed') {
            setBarOpen(true);
          }
          setFocusToken((t) => t + 1);
        }
        return;
      }
      const app = appHotkeyFor(e);
      if (app === null) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      switch (app) {
        case 'find':
          setFindOpen(true);
          setFindToken((t) => t + 1);
          return;
        case 'clear':
          // Clears the scrollback locally; at the prompt Ctrl-L goes through the same route as typed keys (see clearTerminal).
          clearTerminal(effectiveRef.current, handleTerminalInput);
          return;
        case 'zoom-in':
          zoomFont('in');
          return;
        case 'zoom-out':
          zoomFont('out');
          return;
        case 'zoom-reset':
          zoomFont('reset');
          return;
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

  const closeFind = () => {
    setFindOpen(false);
    term.focus();
  };

  const sendRaw = (text: string) => {
    api.writeUser(text).catch(reportError);
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

  // Escape in the bar gives the keyboard back to the terminal. On the alternate screen it also closes the strip.
  const closeBar = () => {
    setBarOpen(false);
    term.focus();
  };

  // Starts a new shell after the old one exited. The terminal is cleared first, so nothing from the old shell
  // shows up in the new one. Focus moves to the terminal when the new shell starts (see focusAfter).
  const restartShell = () => {
    if (restarting) {
      return;
    }
    setRestarting(true);
    resetForNewSession();
    dispatchTranscript({ type: 'session_reset' });
    setAgentControl(NO_CONTROL);
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
          inputState={effective}
          onInput={handleTerminalInput}
          onAltChange={(active) => dispatchInput({ type: 'alt', active })}
        />
        {/* Shown while the agent has the shell. Never takes clicks; sits above the terminal background, below its decorations. */}
        <div className={borderActive(agentControl) ? 'agent-control is-active' : 'agent-control'} aria-hidden="true">
          <div className="agent-control-glow" />
        </div>
        {findOpen && <FindBar focusToken={findToken} onClose={closeFind} />}
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
        focusToken={focusToken}
        onFocusChange={setBarOpen}
        onRestart={restartShell}
        restarting={restarting}
        onAgent={onAgent}
        onInterrupt={() => sendRaw(CTRL_C)}
        onApproveFirst={approveFirst}
        onDenyFirst={denyFirst}
        onClose={closeBar}
      />
    </div>
  );
}
