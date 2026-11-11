import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState } from 'react';
import type { RefObject } from 'react';
import './App.css';
import { api, errorText } from './lib/api';
import { connect, reportError, restart, setSessionNonces, subscribe } from './lib/session';
import type { Incoming } from './lib/session';
import { clearTerminal, currentFontSize, fitToContainer, markConnected, marks, onFontSizeChange, receiveBytes, resetForNewSession, setFontSize, term, terminalSize, zoomFont } from './lib/terminal';
import { appHotkeyFor } from './lib/appHotkeys';
import { effectiveState, focusAfter, inputReducer, INITIAL_INPUT, isFocusBarHotkey, routeFor } from './lib/inputState';
import type { BarMode, Route } from './lib/inputState';
import { agentDialogPhase, agentIntegratingQuietly, agentRequestAfter, agentRequestClosedBySession, agentRunning, canIntegrate, nestedLevels } from './lib/integrate';
import type { AgentRequest, IntegratePhase } from './lib/integrate';
import { agentControlAfter, borderLevel, NO_CONTROL } from './lib/agentControl';
import type { AgentControl, BorderLevel } from './lib/agentControl';
import { EMPTY_TRANSCRIPT, firstPendingApproval, isBusy, transcriptReducer } from './lib/transcript';
import { isSideDock, loadPanelLayout, panelReducer, panelView, savePanelLayout } from './lib/panelLayout';
import type { Container } from './lib/panelLayout';
import { defaultSizeStorage } from './lib/zoom';
import { itemsFromMessages } from './lib/historyView';
import { createPatchSaver, fontPatch, panelPatch, reconcileFont, reconcileGroupBy, reconcilePanel, shouldRestoreLast } from './lib/uiState';
import type { Cwd, HistoryGroupBy, InputState, IntegrationStatus, LevelInfo, LlmStatus, LoadedConversation, ObserverStatus, SettingsStateView, Status, UiState, UiStatePatch, WireEvent } from './lib/types';
import { recoveryKind } from './lib/settingsRecovery';
import { TerminalView } from './components/TerminalView';
import { InputBar } from './components/InputBar';
import { Transcript } from './components/Transcript';
import { StatusChip } from './components/StatusChip';
import { FindBar } from './components/FindBar';
import { PanelDivider } from './components/PanelDivider';
import { AgentPanel, Workspace } from './components/AgentPanel';
import { HistoryView } from './components/HistoryView';
import { SettingsDialog } from './components/SettingsDialog';
import { SettingsBanner } from './components/SettingsBanner';
import { AutoApproveNotice } from './components/AutoApproveNotice';
import { IntegrateDialog } from './components/IntegrateDialog';

const CTRL_C = '\u0003';

/** Writes UI state changes to the file, a short while after the last one (see uiState.ts). Failures are ignored: the state still lives in localStorage. */
const uiSaver = createPatchSaver((patch: UiStatePatch) => {
  api.patchUiState(patch).catch(() => undefined);
});

export default function App() {
  const [input, dispatchInput] = useReducer(inputReducer, INITIAL_INPUT);
  const effective = effectiveState(input);
  const route = routeFor(effective, input.foreground);
  // The Integrate dialog's phase. While the handshake runs (`running`) no key goes to the shell and the bar takes none: the
  // backend is typing the loader and the blob, and a key typed then would land in the middle of them.
  const [integrate, setIntegrate] = useState<IntegratePhase>({ kind: 'closed' });
  // The agent's request to integrate the shell (request_integration): a question, a running handshake (in the dialog, or
  // quietly in auto mode), or its result. The keys are locked while it runs, as for the user's own integration.
  const [agentReq, setAgentReq] = useState<AgentRequest | null>(null);
  const locked = integrate.kind === 'running' || agentRunning(agentReq);
  // The dialog's phase for the agent's request (null: the dialog shows nothing for it, as in auto mode).
  const agentPhase = agentDialogPhase(agentReq);
  const keyRoute: Route = locked ? { ...route, keys: 'drop' } : route;
  // The nested shell levels, outermost first (the local one is not listed). The last is the shell on top.
  const [levels, setLevels] = useState<LevelInfo[]>([]);
  // Approve-all is on, but it was switched on in another shell level, so it is off in the shell on top.
  const [autoSuspended, setAutoSuspended] = useState(false);

  // The bar counts as open while its input has focus, or the hotkey asked for it. Only the alternate screen cares: an
  // open bar stays as the chat there, so a program starting or ending never takes the input's focus away.
  const [barOpen, setBarOpen] = useState(false);
  const barMode: BarMode = route.bar === 'collapsed' && barOpen ? 'chat' : route.bar;

  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, EMPTY_TRANSCRIPT);
  // The agent panel's dock, sizes and open state. Saved on every change (see the effect below).
  const [panel, dispatchPanel] = useReducer(panelReducer, undefined, () => loadPanelLayout(defaultSizeStorage()));
  const workspaceRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const workspace = useElementSize(workspaceRef, { width: window.innerWidth, height: window.innerHeight });
  const panelBox = useElementSize(panelRef, { width: 0, height: 0 });
  // Bumped when a drag or key step ends, so the terminal refits once the new size is on screen.
  const [settle, setSettle] = useState(0);
  const settleLayout = useCallback(() => setSettle((s) => s + 1), []);
  const [integration, setIntegration] = useState<IntegrationStatus>('booting');
  const [observer, setObserver] = useState<ObserverStatus>('ok');
  const [cwd, setCwd] = useState<Cwd | null>(null);
  const [autoApprove, setAutoApprove] = useState(false);
  const [llm, setLlm] = useState<LlmStatus>({ enabled: false, model: null });
  const [focusToken, setFocusToken] = useState(0);
  const [restarting, setRestarting] = useState(false);
  // The agent's hold on the shell: an approved call executing, or a command it started still running (see
  // agentControl.ts). Drives the green border around the terminal.
  const [agentControl, setAgentControl] = useState<AgentControl>(NO_CONTROL);
  // The find bar is mounted while open. Its token moves focus into it when Cmd+F is pressed again.
  const [findOpen, setFindOpen] = useState(false);
  const [findToken, setFindToken] = useState(0);
  // The AI settings dialog (Cmd+, / Ctrl+, or the gear button in the status strip).
  const [settingsOpen, setSettingsOpen] = useState(false);
  // The state of settings.json, from the status. The banner in the chat view can be dismissed until the next status says so again.
  const [settingsState, setSettingsState] = useState<SettingsStateView | null>(null);
  const [settingsBannerHidden, setSettingsBannerHidden] = useState(false);
  // Which view the agent panel's body shows, the id of the live conversation, and how History is grouped.
  const [panelBody, setPanelBody] = useState<'chat' | 'history'>('chat');
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [historyGroupBy, setHistoryGroupBy] = useState<HistoryGroupBy>('date');
  // The UI state file, read at startup. Nothing is written to it until that read has been reconciled (see uiReady).
  const uiLoaded = useRef<Promise<UiState | null> | null>(null);
  const uiReady = useRef(false);
  const restoring = useRef(false);
  // Counts the UI's own changes to the live conversation or the transcript: a send, a new conversation, an open. A load
  // that was started before a change compares it when it returns, so a late result never lands on a newer state.
  const epoch = useRef(0);

  /** Saves a UI state change to the file, once the file has been reconciled. Before that, the change is only in localStorage. */
  const persist = (patch: UiStatePatch) => {
    if (uiReady.current) {
      uiSaver.queue(patch);
    }
  };

  // Read by long-lived listeners (keystrokes, hotkey), so they always see the current route.
  const routeRef = useRef(keyRoute);
  useEffect(() => {
    routeRef.current = keyRoute;
  });
  // Read by the Cmd+K listener, which is registered once.
  const effectiveRef = useRef(effective);
  useEffect(() => {
    effectiveRef.current = effective;
  });
  // Read by the Ctrl/Cmd+Shift+A listener, which is registered once.
  const layoutRef = useRef(panel);
  useEffect(() => {
    layoutRef.current = panel;
  });
  // Read by the keydown listener, which is registered once: while the settings are open, it takes no shortcut but its own.
  const settingsOpenRef = useRef(false);
  useEffect(() => {
    settingsOpenRef.current = settingsOpen;
  });
  // Read after the awaits of the startup restore, so it sees the transcript as it is then.
  const transcriptRef = useRef(transcript);
  useEffect(() => {
    transcriptRef.current = transcript;
  });
  const clientIds = useRef(0);
  const prevEffective = useRef<InputState>(effective);

  const applyStatus = useCallback((s: Status) => {
    dispatchInput({ type: 'rust_state', state: s.state, foreground: s.foreground ?? null });
    setIntegration(s.integration);
    setObserver(s.observer);
    setCwd(s.cwd);
    setAutoApprove(s.auto_approve);
    setAutoSuspended(s.auto_approve_suspended);
    setLevels(s.levels);
    setSessionNonces(s.nonces);
    dispatchTranscript({ type: 'levels_changed', depth: s.depth });
    setLlm(s.llm);
    setSettingsState(s.settings_state ?? null);
    if (recoveryKind(s.settings_state) !== 'ok') {
      setSettingsBannerHidden(false);
    }
    setAgentControl((c) => ({ ...c, call: s.agent_control }));
    setConversationId(s.conversation_id);
    dispatchTranscript({ type: 'sync_busy', busy: s.agent_busy });
    if (s.observer === 'desynced') {
      marks.observerDesynced();
    }
  }, []);

  /** Takes the status snapshot again, e.g. after the live conversation changed. */
  const refreshStatus = () => {
    api.getStatus().then(applyStatus).catch(reportError);
  };

  /**
   * Reads approve-all as the shell on top sees it. Only its two fields change: a levels event is not a full status read (see
   * applyStatus, and the busy state it would reset).
   */
  const refreshAutoState = useCallback(() => {
    api.autoApproveState().then(
      (s) => {
        setAutoApprove(s.auto);
        setAutoSuspended(s.suspended);
      },
      () => undefined,
    );
  }, []);

  /** Reads only the live conversation's id again. The busy state stays the transcript's own, so a send still in transit is not lost. */
  const syncConversationId = () => {
    api.getStatus().then((s) => setConversationId(s.conversation_id)).catch(reportError);
  };

  // The settings refresh's guards: a response is ignored after unmount, and when a newer refresh has started since.
  const settingsAlive = useRef(true);
  const settingsRequest = useRef(0);
  useEffect(() => {
    settingsAlive.current = true;
    return () => {
      settingsAlive.current = false;
    };
  }, []);

  /**
   * Reads the settings state again from the status, so a settings.json fixed by hand shows in the chip and the banner without
   * a restart or Reload. It sets only the settings fields (the state, the banner's visibility as applyStatus does, and the model
   * the chip shows). It never touches the busy state, the conversation or the input, so it is safe while a send is in transit.
   * Errors are ignored: the next refresh tries again.
   */
  const refreshSettingsState = useCallback(() => {
    settingsRequest.current += 1;
    const request = settingsRequest.current;
    api.getStatus().then(
      (s) => {
        if (!settingsAlive.current || request !== settingsRequest.current) {
          return;
        }
        setLlm(s.llm);
        setSettingsState(s.settings_state ?? null);
        if (recoveryKind(s.settings_state) !== 'ok') {
          setSettingsBannerHidden(false);
        }
      },
      () => undefined,
    );
  }, []);

  /** Opens the AI settings, and reads the settings state again on the way in, so the chip and the banner match the file. */
  const openSettings = () => {
    setSettingsOpen(true);
    refreshSettingsState();
  };

  /** Closes the AI settings, and reads the settings state again: the dialog may have changed the file. */
  const closeSettings = () => {
    setSettingsOpen(false);
    refreshSettingsState();
  };

  // The UI state file: read once, reconciled with the localStorage values, and kept in step after that. The file wins when it
  // has a valid value; a missing value is filled in from localStorage (see uiState.ts). With no file (no store), nothing changes.
  useEffect(() => {
    const load = api.getUiState().catch(() => null);
    uiLoaded.current = load;
    void load.then((remote) => {
      if (remote === null) {
        return;
      }
      uiReady.current = true;
      const panelSync = reconcilePanel(layoutRef.current, remote.panel);
      if (panelSync.apply !== null) {
        dispatchPanel({ type: 'replace', layout: panelSync.apply });
      }
      if (panelSync.migrate !== null) {
        persist(panelPatch(panelSync.migrate));
      }
      const fontSync = reconcileFont(currentFontSize(), remote.font_size);
      if (fontSync.apply !== null) {
        setFontSize(fontSync.apply, false);
      }
      if (fontSync.migrate !== null) {
        persist(fontPatch(fontSync.migrate));
      }
      const group = reconcileGroupBy(remote.history_group_by);
      if (group !== null) {
        setHistoryGroupBy(group);
      }
    });
    const unsubscribeFont = onFontSizeChange((size) => persist(fontPatch(size)));
    const flushOnHide = () => uiSaver.flush();
    window.addEventListener('pagehide', flushOnHide);
    return () => {
      unsubscribeFont();
      window.removeEventListener('pagehide', flushOnHide);
    };
  }, []);

  // Connect, then load the snapshot. The snapshot covers events that arrived before this listener was up.
  useEffect(() => {
    /**
     * Reopens the last conversation when the transcript is empty and no turn is running (so a send cannot race it). A failure
     * (not found, unreadable) forgets the last conversation, quietly. Nothing is shown. If the user acted while the load was
     * running (a send, a new conversation, an open), their state stands: the restored messages are not shown, and the status
     * is read again so the live conversation id matches the backend.
     */
    const restoreLast = async (status: Status) => {
      if (restoring.current) {
        return;
      }
      restoring.current = true;
      const remote = await (uiLoaded.current ?? Promise.resolve(null));
      const lastId = remote?.last_conversation_id ?? null;
      const current = transcriptRef.current;
      if (lastId === null || !shouldRestoreLast({ lastId, itemCount: current.items.length, transcriptBusy: isBusy(current), statusBusy: status.agent_busy })) {
        return;
      }
      const started = epoch.current;
      try {
        const loaded = await api.loadConversation(lastId);
        if (epoch.current !== started || transcriptRef.current.items.length > 0) {
          syncConversationId();
          return;
        }
        epoch.current += 1;
        dispatchTranscript({ type: 'conversation_loaded', items: itemsFromMessages(loaded.messages), todos: loaded.todos });
        setConversationId(loaded.id);
      } catch {
        if (transcriptRef.current.items.length === 0) {
          api.patchUiState({ last_conversation_id: null }).catch(() => undefined);
        }
      }
    };

    const handleEvent = (ev: WireEvent) => {
      setAgentControl((c) => agentControlAfter(c, ev));
      switch (ev.type) {
        case 'state':
          dispatchInput({ type: 'rust_state', state: ev.state, foreground: ev.foreground ?? null });
          // A shell that exited ends a question still waiting (see agentRequestAfter).
          setAgentReq((cur) => agentRequestAfter(cur, ev));
          return;
        case 'integration':
          setIntegration(ev.status);
          return;
        case 'integration_request':
          setAgentReq((cur) => agentRequestAfter(cur, ev));
          // The agent's question is the one that counts: a consent dialog the user opened for this shell gives way to it.
          if (ev.phase === 'asking') {
            setIntegrate({ kind: 'closed' });
          }
          return;
        case 'done':
          // The turn ended: a question still waiting for an answer is closed (nobody can answer it now).
          setAgentReq((cur) => agentRequestAfter(cur, ev));
          dispatchTranscript({ type: 'agent_event', event: ev });
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
          // Closes the interactive group the command opened on, if there is one (see transcript.ts).
          dispatchTranscript({ type: 'record_finished', record: ev.record });
          return;
        case 'levels':
          // Rust pushed, popped or pending-ed a level. The nonces are taken first, so the bytes after this event are checked
          // against them. The location and approve-all follow the new top level.
          setSessionNonces(ev.nonces);
          setLevels(nestedLevels(ev.levels));
          // The depth ends an interactive group a level was pushed above (see transcript.ts). The same for an integration the
          // agent asked for and one the user started.
          dispatchTranscript({ type: 'levels_changed', depth: ev.levels.length });
          refreshAutoState();
          return;
        default:
          dispatchTranscript({ type: 'agent_event', event: ev, at: Date.now() });
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
      .then((status) => {
        applyStatus(status);
        void restoreLast(status);
      })
      .then(() => fitToContainer())
      .catch(reportError);
    return unsubscribe;
  }, [applyStatus, refreshAutoState]);

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

  // Saved on every change. A drag saves on each move; the write is a few hundred bytes. The UI state file gets the same
  // change, debounced (see uiSaver).
  useEffect(() => {
    savePanelLayout(panel, defaultSizeStorage());
    persist(panelPatch(panel));
  }, [panel]);

  // Refits the terminal once a dock change, a rail change or a drag's end is on screen. The fit reads the settled size
  // of .term-host. The ResizeObserver on .term-host refits too (with its own debounce); this one makes the final fit
  // land on the final layout. Runs after React has committed the change, so it never measures the old layout.
  useEffect(() => {
    const frame = requestAnimationFrame(() => void fitToContainer());
    return () => cancelAnimationFrame(frame);
  }, [panel.dock, panel.open, settle]);

  // Ctrl+Shift+A (Cmd+Shift+A) focuses the agent bar, from anywhere it is offered. Cmd+F, Cmd+K and Cmd+=/-/0 (see
  // appHotkeys.ts) are handled here too. Capture phase, so xterm does not also get the key, and a handled key is
  // stopped here so it never reaches the PTY.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // While the AI settings are open they own the keyboard: only their own hotkey is taken here, so no shortcut reaches
      // the terminal or moves focus out of the dialog.
      if (settingsOpenRef.current) {
        if (appHotkeyFor(e) === 'settings') {
          e.preventDefault();
          e.stopPropagation();
        }
        return;
      }
      if (isFocusBarHotkey(e)) {
        e.preventDefault();
        e.stopPropagation();
        // A side panel collapsed to the rail opens first. Both state changes land in one render, so the input is
        // visible when the focus effect in InputBar runs.
        const { dock, open } = layoutRef.current;
        if (!open && isSideDock(dock)) {
          dispatchPanel({ type: 'open', open: true });
        }
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
        case 'settings':
          openSettings();
          return;
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  // While settings.json cannot be used, returning to the window reads it again, so a file fixed by hand is picked up. Not
  // while it is ok, and not for last good: the backend leaves that only by a Reload.
  const settingsKind = recoveryKind(settingsState);
  useEffect(() => {
    if (settingsKind === 'ok') {
      return;
    }
    const onFocus = () => refreshSettingsState();
    const onVisibility = () => {
      if (document.visibilityState === 'visible') {
        refreshSettingsState();
      }
    };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [settingsKind, refreshSettingsState]);

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
    epoch.current += 1;
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

  // Clears the agent's conversation. The backend refuses while the agent is busy; the button is also disabled then. On
  // error the reason is shown in the transcript, and the conversation stays as it was.
  const newConversation = () => {
    epoch.current += 1;
    api.newConversation().then(
      () => {
        dispatchTranscript({ type: 'conversation_reset' });
        // The new conversation has a new id, which the history view marks as current.
        refreshStatus();
      },
      (e) => dispatchTranscript({ type: 'notice', text: errorText(e) }),
    );
  };

  /**
   * A saved conversation was opened from the History view: it becomes the live transcript, and the chat is shown with the
   * input focused. The backend refuses the load while the agent is busy. A send, a new conversation or another open that
   * happened while this one loaded is not overwritten: the result is refused, and the status is read again.
   * Resolves with the reason it was refused, or null when it was opened.
   */
  const openConversation = async (id: string): Promise<string | null> => {
    const started = epoch.current;
    let loaded: LoadedConversation;
    try {
      loaded = await api.loadConversation(id);
    } catch (e) {
      return errorText(e);
    }
    if (epoch.current !== started || isBusy(transcriptRef.current)) {
      syncConversationId();
      return 'The chat changed while this conversation was loading, so it was not opened. Open it again.';
    }
    epoch.current += 1;
    dispatchTranscript({ type: 'conversation_loaded', items: itemsFromMessages(loaded.messages), todos: loaded.todos });
    setConversationId(loaded.id);
    setPanelBody('chat');
    setFocusToken((t) => t + 1);
    refreshStatus();
    return null;
  };

  /** The live conversation was deleted from the History view: the backend started a new one, so the chat is reset to match. */
  const liveDeleted = () => {
    epoch.current += 1;
    dispatchTranscript({ type: 'conversation_reset' });
    setConversationId(null);
    refreshStatus();
  };

  /** Moves focus to the History header button, so it is not lost when the view closes. */
  const focusHistoryButton = () => document.getElementById('history-button')?.focus();

  /** The History button. It opens the panel first if it is collapsed, so the view is visible. Closing returns focus to it. */
  const toggleHistory = () => {
    if (!panel.open) {
      dispatchPanel({ type: 'open', open: true });
    }
    if (panelBody === 'history') {
      focusHistoryButton();
    }
    setPanelBody((b) => (b === 'history' ? 'chat' : 'history'));
  };

  /** Esc in the History view closes it, and focus goes back to the button. Opening a row does not come through here. */
  const closeHistory = () => {
    focusHistoryButton();
    setPanelBody('chat');
  };

  const setGroupBy = (groupBy: HistoryGroupBy) => {
    setHistoryGroupBy(groupBy);
    persist({ history_group_by: groupBy });
  };

  const answer = (callId: string, approve: boolean) => {
    dispatchTranscript({ type: 'approval_answering', callId, approve });
    api.approveTool(callId, approve).catch((e) => {
      dispatchTranscript({ type: 'approval_failed', callId, message: errorText(e) });
    });
  };

  const pendingApproval = firstPendingApproval(transcript);
  const busy = isBusy(transcript);
  // The border is 'working' for the whole agent turn, but an exited shell takes it off even if a turn is still open.
  const border: BorderLevel = borderLevel(agentControl, busy && effective !== 'exited');
  const borderClass: Record<BorderLevel, string> = {
    off: 'agent-control',
    working: 'agent-control is-working',
    control: 'agent-control is-active',
  };

  // On turns it off. Off, or on in another shell (suspended), turns it on for this shell: the backend ties it to the top level.
  const toggleAutoApprove = () => {
    const next = !(autoApprove && !autoSuspended);
    api.setAutoApprove(next).then(() => {
      setAutoApprove(next);
      refreshAutoState();
    }, reportError);
  };

  /** Opens the consent dialog. Nothing is sent to the shell until the user confirms it there. */
  const openIntegrate = () => setIntegrate({ kind: 'consent' });

  /** Runs the integration, and shows its result in the dialog. The keys stay locked until it answers. */
  const startIntegrate = () => {
    setIntegrate({ kind: 'running' });
    api.integrateShell().then(
      (outcome) => setIntegrate({ kind: 'result', outcome }),
      (e: unknown) => setIntegrate({ kind: 'result', outcome: { outcome: 'failed', hint: errorText(e) } }),
    );
  };

  const closeIntegrate = () => setIntegrate({ kind: 'closed' });

  /** Integrate on the agent's question: the handshake runs with the keys locked, and its `done` event brings the result. */
  const acceptAgent = () => {
    if (agentReq?.kind !== 'asking') {
      return;
    }
    const { callId, label } = agentReq;
    setAgentReq({ kind: 'running', callId, label, shown: true });
    api.answerIntegration(callId, true).catch((e: unknown) => {
      // Refused: no such question is pending (it timed out, or the turn ended). Nothing is running for it.
      setAgentReq((cur) => (cur?.kind === 'running' && cur.callId === callId ? null : cur));
      dispatchTranscript({ type: 'notice', text: errorText(e) });
    });
  };

  /** Decline on the agent's question (Decline, Escape, the backdrop). The dialog closes at once. */
  const declineAgent = () => {
    if (agentReq?.kind !== 'asking') {
      return;
    }
    const { callId } = agentReq;
    setAgentReq(null);
    // A refused answer means the question was already settled (timed out or cut short): the dialog is closed either way.
    api.answerIntegration(callId, false).catch(() => undefined);
  };

  const closeAgent = () => setAgentReq(null);

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
    setAgentReq((cur) => agentRequestClosedBySession(cur));
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

  const view = panelView(panel, workspace);
  // The panel's size on its axis as it is drawn now, read live for a drag's start and for each key.
  const measurePanel = () => {
    const el = panelRef.current;
    if (!el) {
      return 0;
    }
    const box = el.getBoundingClientRect();
    return view.side ? box.width : box.height;
  };

  return (
    <div className="app">
      <StatusChip
        integration={integration}
        observer={observer}
        input={effective}
        foreground={input.foreground}
        cwd={cwd}
        autoApprove={autoApprove}
        autoSuspended={autoSuspended}
        onToggleAutoApprove={toggleAutoApprove}
        location={levels.length > 0 ? levels[levels.length - 1] : null}
        canIntegrate={integrate.kind === 'closed' && agentReq === null && canIntegrate(effective, input.foreground)}
        onIntegrate={openIntegrate}
        integrating={agentIntegratingQuietly(agentReq)}
        llm={llm}
        settingsState={settingsState}
        onOpenSettings={openSettings}
      />
      {/* The terminal is always the first child and the panel the last (see Workspace). The dock only changes the CSS. */}
      <Workspace
        ref={workspaceRef}
        dock={panel.dock}
        terminal={
          <div className="terminal-area">
            <TerminalView
              keysToPty={keyRoute.keys === 'pty'}
              inputState={effective}
              onInput={handleTerminalInput}
              onAltChange={(active) => dispatchInput({ type: 'alt', active })}
            />
            {/* Shown during the agent's turn ('working', softer) and while it holds the shell ('control'). Never takes clicks; sits above the terminal background, below its decorations. */}
            <div className={borderClass[border]} aria-hidden="true">
              <div className="agent-control-glow" />
            </div>
            {findOpen && <FindBar focusToken={findToken} onClose={closeFind} />}
          </div>
        }
        divider={
          view.divider ? (
            <PanelDivider
              dock={panel.dock}
              container={workspace}
              valueNow={view.side ? panelBox.width : panelBox.height}
              measure={measurePanel}
              onResize={(size) => dispatchPanel({ type: 'size', dock: panel.dock, size })}
              onReset={() => dispatchPanel({ type: 'reset_size', dock: panel.dock })}
              onSettle={settleLayout}
            />
          ) : null
        }
        panel={
          <AgentPanel ref={panelRef} view={view} onExpand={() => dispatchPanel({ type: 'toggle' })}>
            <Transcript
              items={transcript.items}
              busy={busy}
              thinking={transcript.thinking}
              todos={transcript.todos}
              open={panel.open}
              onToggle={() => dispatchPanel({ type: 'toggle' })}
              onApprove={answer}
              onNewConversation={newConversation}
              warning={
                autoApprove ? (
                  <AutoApproveNotice suspended={autoSuspended} onTurnOff={toggleAutoApprove} onTurnOnHere={toggleAutoApprove} />
                ) : null
              }
              banner={
                settingsState !== null && recoveryKind(settingsState) !== 'ok' && !settingsBannerHidden ? (
                  <SettingsBanner
                    state={settingsState}
                    onOpenSettings={openSettings}
                    onDismiss={() => {
                      setSettingsBannerHidden(true);
                      // The button unmounts with the banner. Focus goes to the panel's header button, which stays put.
                      document.querySelector<HTMLElement>('.transcript-head')?.focus();
                    }}
                  />
                ) : null
              }
              dock={panel.dock}
              onDock={(dock) => dispatchPanel({ type: 'dock', dock })}
              view={panelBody}
              onToggleHistory={toggleHistory}
              history={
                panelBody === 'history' ? (
                  <HistoryView
                    currentId={conversationId}
                    busy={busy}
                    groupBy={historyGroupBy}
                    onGroupBy={setGroupBy}
                    onOpen={openConversation}
                    onLiveDeleted={liveDeleted}
                    onClose={closeHistory}
                  />
                ) : null
              }
            />
            <InputBar
              mode={barMode}
              hint={locked ? 'Integrating this shell. Keys are not sent until it is done.' : route.hint}
              hintInline={locked ? false : route.hintInline}
              locked={locked}
              hasPendingApproval={pendingApproval !== null}
              busy={busy}
              focusToken={focusToken}
              onFocusChange={setBarOpen}
              onRestart={restartShell}
              restarting={restarting}
              onAgent={onAgent}
              onInterrupt={() => {
                if (!locked) {
                  sendRaw(CTRL_C);
                }
              }}
              onApproveFirst={approveFirst}
              onDenyFirst={denyFirst}
              onClose={closeBar}
            />
          </AgentPanel>
        }
      />
      {settingsOpen && <SettingsDialog onClose={closeSettings} onChanged={refreshStatus} onViewLoaded={refreshSettingsState} />}
      {agentPhase !== null ? (
        <IntegrateDialog key="agent" phase={agentPhase} agent={{ onDecline: declineAgent }} onStart={acceptAgent} onClose={closeAgent} />
      ) : (
        integrate.kind !== 'closed' && <IntegrateDialog key="user" phase={integrate} onStart={startIntegrate} onClose={closeIntegrate} />
      )}
    </div>
  );
}

/**
 * The border-box size of an element, kept current with a ResizeObserver. `initial` is what the first render sees, before
 * the element has been measured (the layout effect measures before the first paint).
 */
function useElementSize(ref: RefObject<HTMLElement | null>, initial: Container): Container {
  const [size, setSize] = useState(initial);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) {
      return;
    }
    const measure = () => {
      const box = el.getBoundingClientRect();
      setSize((prev) => (prev.width === box.width && prev.height === box.height ? prev : { width: box.width, height: box.height }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}
