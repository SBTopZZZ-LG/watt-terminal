import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState, useSyncExternalStore } from 'react';
import type { RefObject } from 'react';
import './App.css';
import { api, CHAT_ID, errorText, SESSION_ID } from './lib/api';
import { showsDisplay } from './lib/approvalCard';
import { copyText } from './lib/clipboard';
import {
  INIT_TEMPLATE,
  PLAN_TEMPLATE,
  REVIEW_TEMPLATE,
  expandTemplate,
  findBuiltin,
  matchModel,
  parseCommand,
  renderContext,
  renderHelp,
  splitModelVariant,
} from './lib/commands';
import type { ModelChoice } from './lib/commands';
import { effectiveVariants } from './lib/settingsForm';
import { reportError, tabs } from './lib/tabManager';
import type { Incoming } from './lib/tabManager';
import { ActiveTabContext } from './lib/activeTab';
import { isSessionEvent } from './lib/tabState';
import { isOwnedByChat } from './lib/tabStrip';
import type { TabController } from './lib/tab';
import type { TabState } from './lib/tabState';
import { appHotkeyFor } from './lib/appHotkeys';
import { findZone } from './lib/findZone';
import { isEditableElement, isSelectAllKey, selectAllTarget, selectContentsOf } from './lib/selectAll';
import { effectiveState, focusAfter, isFocusBarHotkey, routeFor } from './lib/inputState';
import type { BarMode, Route } from './lib/inputState';
import { agentDialogPhase, agentIntegratingQuietly, agentRequestAfter, agentRequestClosedBySession, agentRunning, canIntegrate, integratesLocalShell } from './lib/integrate';
import type { AgentRequest, IntegratePhase } from './lib/integrate';
import { useSettledStrip } from './lib/stripSettle';
import { borderActive, borderLevel } from './lib/agentControl';
import type { BorderLevel } from './lib/agentControl';
import { EMPTY_TRANSCRIPT, firstPendingApproval, hasOlder, isBusy, transcriptReducer } from './lib/transcript';
import type { TranscriptItem } from './lib/transcript';
import { chatBodyShown, loadPanelLayout, panelReducer, panelView, savePanelLayout } from './lib/panelLayout';
import type { Container, PanelAction } from './lib/panelLayout';
import { altPanelStep, INITIAL_ALT_PANEL, layoutToSave } from './lib/altPanel';
import type { AltPanelState } from './lib/altPanel';
import { defaultSizeStorage } from './lib/zoom';
import { agentActivity } from './lib/agentActivity';
import { userAttachments } from './lib/attachments';
import { defaultModelVision } from './lib/modelVision';
import { createPatchSaver, devtoolsPatch, fontPatch, hideMarksPatch, notificationsPatch, panelPatch, queueModePatch, reconcileDevtools, reconcileFont, reconcileGroupBy, reconcileHideMarks, reconcileNotifications, reconcilePanel, reconcileQueueMode, reconcileSuggestPrompts, reconcileThinkingMode, shouldRestoreLast, suggestPromptsPatch, thinkingModePatch } from './lib/uiState';
import { DEFAULT_THINKING_MODE } from './lib/thinking';
import { decideAttention } from './lib/attention';
import type { AttentionEvent } from './lib/attention';
import { onNotificationClick, sendAttention, watchWindowFocus } from './lib/attentionClient';
import { acceptSuggestions, endsWithAgentText, shouldSuggest } from './lib/suggestions';
import { NEW_DRAFT_KEY, forgetAllSuggestions, forgetConversation, forgetEverything, forgetSuggestions, loadSuggestions, saveSuggestions, transcriptSig } from './lib/conversationCache';
import { SEND_WHILE_SWITCHING_NOTICE, switchRefusal } from './lib/switchGuard';
import { drainBatch, enqueue, markSteered, QUEUE_SLOT_POLL_MS, QUEUE_SLOT_WAIT_MS, removeInjected, removeQueued, shouldDrain, takeWithdrawn, unsteer } from './lib/messageQueue';
import type { QueuedMessage } from './lib/messageQueue';
import type { AttachmentInput, HistoryGroupBy, InputState, LlmStatus, LoadedConversation, QueueMode, QuestionAnswerInput, SessionEvent, SettingsStateView, Status, ThinkingMode, UiState, UiStatePatch, WireEvent } from './lib/types';
import { recoveryKind } from './lib/settingsRecovery';
import { TerminalView } from './components/TerminalView';
import { TabStrip } from './components/TabStrip';
import { InputBar } from './components/InputBar';
import { Transcript } from './components/Transcript';
import { TodoPanel } from './components/TodoPanel';
import { ContextMeter } from './components/ContextMeter';
import { StatusChip } from './components/StatusChip';
import { FindBar } from './components/FindBar';
import { ChatFindBar } from './components/ChatFindBar';
import { PanelDivider } from './components/PanelDivider';
import { AgentPanel, Workspace } from './components/AgentPanel';
import { AgentActivityToast } from './components/AgentActivityToast';
import { ApprovalToast } from './components/ApprovalToast';
import { HistoryView } from './components/HistoryView';
import { HelpDialog } from './components/HelpDialog';
import type { HelpLoad } from './components/HelpDialog';
import { SettingsDialog } from './components/SettingsDialog';
import { SettingsBanner } from './components/SettingsBanner';
import { IntegrateDialog } from './components/IntegrateDialog';
import { Tooltip } from './components/Tooltip';
import { ContextMenu } from './components/ContextMenu';

const CTRL_C = '\u0003';

/** Writes UI state changes to the file, a short while after the last one (see uiState.ts). Failures are ignored: the state still lives in localStorage. */
const uiSaver = createPatchSaver((patch: UiStatePatch) => {
  api.patchUiState(patch).catch(() => undefined);
});

/** The per-conversation memory in localStorage: drafts and saved follow-up prompts (see conversationCache.ts). */
const cacheStorage = defaultSizeStorage();

/**
 * The terminal an agent event is about: the one it names (`terminal`, sent when the chat has two or more), else the chat's
 * first terminal. Null when the chat has none.
 */
function agentTarget(ev: WireEvent): TabController | null {
  if ('terminal' in ev && ev.terminal !== undefined) {
    return tabs.get(ev.terminal) ?? null;
  }
  return tabs.list().find((t) => isOwnedByChat(t.getSnapshot().owner, CHAT_ID)) ?? null;
}

export default function App() {
  // The terminal the UI shows: the registry's active one (see tabManager.ts). Its state (input, hold, levels, ...) is read
  // from its controller (see tab.ts). The first terminal is made here so there is one to show before the backend's list.
  const [firstTab] = useState(() => tabs.ensure(SESSION_ID));
  const registry = useSyncExternalStore(tabs.subscribeStore, tabs.getSnapshot);
  const tab = (registry.activeId === null ? undefined : tabs.get(registry.activeId)) ?? firstTab;
  const { input, integration, observer, cwd, levels, agentControl, restarting } = useTabState(tab);
  const effective = effectiveState(input);
  const route = routeFor(effective, input.foreground);
  // What the status strip shows of the state and the foreground hint: the prompt is held while a quick command runs (see stripSettle.ts).
  const strip = useSettledStrip(effective, input.foreground);
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
  // The consent says "this machine" for a bare shell typed here (see integratesLocalShell). Read per render: the foreground is live.
  const localShell = integratesLocalShell(input.foreground, levels);
  // Approve-all is on, but it was switched on in another shell level, so it is off in the shell on top.
  const [autoSuspended, setAutoSuspended] = useState(false);

  // The bar counts as open while its input has focus, or the hotkey asked for it. Only the alternate screen cares: an
  // open bar stays as the chat there, so a program starting or ending never takes the input's focus away.
  const [barOpen, setBarOpen] = useState(false);
  const barMode: BarMode = route.bar === 'collapsed' && barOpen ? 'chat' : route.bar;

  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, EMPTY_TRANSCRIPT);
  // The agent panel's dock, sizes and open state. Saved on every change (see the effect below).
  const [panel, dispatchPanel] = useReducer(panelReducer, undefined, () => loadPanelLayout(defaultSizeStorage()));
  // The panel's auto-hide while a full-screen program runs (see altPanel.ts). A ref: the listeners read it as it is now.
  const altPanel = useRef<AltPanelState>(INITIAL_ALT_PANEL);
  /** A panel action the user took (a button, the hotkey, a toast, History). Inside a full-screen run it takes control of the panel. */
  const userPanel = (action: PanelAction) => {
    altPanel.current = altPanelStep(altPanel.current, { type: 'user' }).state;
    dispatchPanel(action);
  };
  const workspaceRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const workspace = useElementSize(workspaceRef, { width: window.innerWidth, height: window.innerHeight });
  const panelBox = useElementSize(panelRef, { width: 0, height: 0 });
  // Bumped when a drag or key step ends, so the terminal refits once the new size is on screen.
  const [settle, setSettle] = useState(0);
  const settleLayout = useCallback(() => setSettle((s) => s + 1), []);
  const [autoApprove, setAutoApprove] = useState(false);
  const [llm, setLlm] = useState<LlmStatus>({ enabled: false, model: null });
  const [focusToken, setFocusToken] = useState(0);
  // The find bar is mounted while open. Its token moves focus into it when Cmd+F is pressed again.
  const [findOpen, setFindOpen] = useState(false);
  const [findToken, setFindToken] = useState(0);
  // The chat find (Cmd+F with focus in the agent panel) is independent of the terminal find: both can be open. Esc returns
  // focus to the element that was focused when it opened (chatFindOrigin).
  const [chatFindOpen, setChatFindOpen] = useState(false);
  const [chatFindToken, setChatFindToken] = useState(0);
  const chatFindOrigin = useRef<Element | null>(null);
  // The AI settings dialog (Cmd+, / Ctrl+, or the gear button in the status strip).
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Bare `/model` opens Settings with the default-model select focused. Any other open leaves focus alone.
  const [settingsFocusModel, setSettingsFocusModel] = useState(false);
  // The `/help` dialog. It loads fresh on each open and never writes to the transcript.
  const [helpOpen, setHelpOpen] = useState(false);
  const [helpState, setHelpState] = useState<HelpLoad>({ kind: 'loading' });
  // The state of settings.json, from the status. The banner in the chat view can be dismissed until the next status says so again.
  const [settingsState, setSettingsState] = useState<SettingsStateView | null>(null);
  const [settingsBannerHidden, setSettingsBannerHidden] = useState(false);
  // Which view the agent panel's body shows, the id of the live conversation, and how History is grouped.
  const [panelBody, setPanelBody] = useState<'chat' | 'history'>('chat');
  const [conversationId, setConversationId] = useState<string | null>(null);
  // A side command (`/btw`, `/compact`, `/fork`) is running: one model call outside a turn, or none at all. The bar
  // and History stay busy until it answers, and a send is refused so the text is kept.
  const [sideBusy, setSideBusy] = useState(false);
  // Messages typed while the agent works: steered into the running turn, or queued and sent after it (see messageQueue.ts). Not kept across a restart. The ref is the
  // same list for the steps that run outside a render. `queueGen` moves when the conversation changes, so a send in flight
  // never lands in another one. `queueMode` is the saved setting: all together, or one per turn.
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const queueRef = useRef<QueuedMessage[]>([]);
  const queueSeq = useRef(0);
  const queueGen = useRef(0);
  const draining = useRef(false);
  const drainAgain = useRef(false);
  const [queueMode, setQueueMode] = useState<QueueMode>('all');
  // Queued messages taken back by a Stop or a restart. The input takes them as an unqueue does (InputBar's restoreTick).
  const [restoreTick, setRestoreTick] = useState(0);
  const returned = useRef<QueuedMessage[]>([]);
  // The side-command flag for the status reader, which is registered once (see applyStatus).
  const sideBusyRef = useRef(false);
  useEffect(() => {
    sideBusyRef.current = sideBusy;
  });
  // How reasoning blocks show (the Reasoning setting). Auto until the file says otherwise.
  const [thinkingMode, setThinkingMode] = useState<ThinkingMode>(DEFAULT_THINKING_MODE);
  // Bumped when the live conversation is renamed, so an open HistoryView remounts and refetches its rows.
  const [historyKey, setHistoryKey] = useState(0);
  const [historyGroupBy, setHistoryGroupBy] = useState<HistoryGroupBy>('date');
  // Hides the terminal's separators and agent labels (CSS on the root element, see App.css). Hidden until the file says otherwise.
  const [hideMarks, setHideMarks] = useState(true);
  useEffect(() => {
    document.documentElement.dataset.hideMarks = String(hideMarks);
  }, [hideMarks]);
  // Follow-up prompts after a reply (see suggestions.ts). On until the file says otherwise. `suggestGen` moves whenever the
  // suggestions no longer follow the reply (a send, a turn, a conversation change, the setting, a click), so a late answer is
  // dropped. One request at a time; a trigger during one is run again after it.
  const [suggestPrompts, setSuggestPrompts] = useState(true);
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const suggestGen = useRef(0);
  const suggesting = useRef(false);
  const suggestAgain = useRef(false);
  // Bumped when a conversation is deleted, so the input bar drops its unsent text for it instead of saving it under the deleted id.
  const [discardDraft, setDiscardDraft] = useState(0);
  // True while an open or a new conversation is in flight. Sends and queue drains wait, so no message lands in the other one.
  const switching = useRef(false);
  // Desktop notifications when the agent is done or needs the user while Watt is in the background (see attention.ts). On
  // until the file says otherwise. `windowFocused` is the native focus state, kept by a listener; it starts true, so nothing
  // is sent before the state is known. `attentionKey` is the last question or approval already handled, so each shows once.
  const [notifications, setNotifications] = useState(true);
  // The layout inspector setting (Terminal tab, Developer). Off until the file says otherwise; the backend refuses to open
  // DevTools while it is off, and this only decides whether the right-click menu offers the item.
  const [devtools, setDevtools] = useState(false);
  const windowFocused = useRef(true);
  const attentionKey = useRef<string | null>(null);
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
  // Read by the suggestion request and the saved-suggestions steps, which run after awaits.
  const conversationRef = useRef<string | null>(null);
  useEffect(() => {
    conversationRef.current = conversationId;
  });
  const clientIds = useRef(0);
  const prevEffective = useRef<InputState>(effective);

  /**
   * Applies a status read for `target`, the terminal it was read for: its shell fields go to that terminal, the chat's to the
   * UI. Stable (no closure over the active terminal), so the connection effect is not re-run by a switch.
   */
  const applyStatus = useCallback((s: Status, target: TabController) => {
    target.applyStatus(s);
    setAutoApprove(s.auto_approve);
    setAutoSuspended(s.auto_approve_suspended);
    dispatchTranscript({ type: 'levels_changed', depth: s.depth });
    setLlm(s.llm);
    setSettingsState(s.settings_state ?? null);
    if (recoveryKind(s.settings_state) !== 'ok') {
      setSettingsBannerHidden(false);
    }
    setConversationId(s.conversation_id);
    // The backend's busy also covers a side command (btw, suggest_name), which never sends `done`. While one runs, the status
    // says nothing about the transcript's turn, so it is not synced; once it ends, the next status reads idle.
    if (!sideBusyRef.current) {
      dispatchTranscript({ type: 'sync_busy', busy: s.agent_busy });
    }
  }, []);

  /** Takes the status snapshot again, e.g. after the live conversation changed. Resolves once it is applied. */
  const refreshStatus = (): Promise<void> =>
    api.getStatus(tab.id).then((s) => applyStatus(s, tab)).catch(reportError);

  /**
   * Re-reads the control hold after `done`. `done` is emitted by `run_turn` before the worker's `OnDrop` calls
   * `end_turn`, so the first fetch can still see `agent_busy: true` and `settleControl` keeps the stale command id.
   * Retry while busy (up to 4 retries, ~250ms apart) so the later read settles it; errors are ignored.
   */
  const refreshControl = useCallback(function refresh(target: TabController, attempt = 0): void {
    api.getStatus(target.id).then(
      (s) => {
        target.applyControl(s);
        if (s.agent_busy && attempt < 4) {
          setTimeout(() => refresh(target, attempt + 1), 250);
        }
      },
      () => undefined,
    );
  }, []);

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
  const syncConversationId = (): Promise<void> => api.getStatus().then((s) => setConversationId(s.conversation_id)).catch(reportError);

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
    setSettingsFocusModel(false);
    setSettingsOpen(true);
    refreshSettingsState();
  };

  /** Bare `/model`: the settings open with the default-model select focused, so Space opens its list. */
  const openSettingsForModel = () => {
    setSettingsFocusModel(true);
    setSettingsOpen(true);
    refreshSettingsState();
  };

  /** Closes the AI settings, and reads the settings state again: the dialog may have changed the file. */
  const closeSettings = () => {
    setSettingsFocusModel(false);
    setSettingsOpen(false);
    refreshSettingsState();
  };

  /** Reads the command catalog again for the `/help` dialog. Errors show in the dialog, never the transcript. */
  const loadHelp = () => {
    setHelpState({ kind: 'loading' });
    api.listCommands(tab.id).then(
      (catalog) => setHelpState({ kind: 'ready', text: renderHelp(catalog) }),
      (e: unknown) => setHelpState({ kind: 'error', message: errorText(e) }),
    );
  };

  /** Closes the `/help` dialog. Focus goes back to the bar through the token. */
  const closeHelp = () => {
    setHelpOpen(false);
    setFocusToken((t) => t + 1);
  };

  // Whether the selected model takes images. The status names the model; the saved settings say what it takes. Read again when
  // the selected model changes and when the settings dialog closes. Unknown (no default, or a failed read) is null.
  const [modelVision, setModelVision] = useState<boolean | null>(null);
  useEffect(() => {
    let live = true;
    api.getSettings().then(
      (view) => {
        if (live) {
          setModelVision(defaultModelVision(view.settings));
        }
      },
      () => {
        if (live) {
          setModelVision(null);
        }
      },
    );
    return () => {
      live = false;
    };
  }, [llm.enabled, llm.model, llm.provider, settingsOpen]);

  // Fades the loading splash (see public/splash.css) once React has mounted, then removes it. StrictMode mounts
  // twice: the cleanup clears the first timer, and the second pass re-adds the class and re-arms the removal.
  useEffect(() => {
    const el = document.getElementById('splash');
    if (el === null) {
      return;
    }
    el.classList.add('splash-done');
    const timer = setTimeout(() => el.remove(), 250);
    return () => clearTimeout(timer);
  }, []);

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
      const fontSync = reconcileFont(tabs.currentFontSize(), remote.font_size);
      if (fontSync.apply !== null) {
        tabs.setFontSize(fontSync.apply, false);
      }
      if (fontSync.migrate !== null) {
        persist(fontPatch(fontSync.migrate));
      }
      const group = reconcileGroupBy(remote.history_group_by);
      if (group !== null) {
        setHistoryGroupBy(group);
      }
      const queued = reconcileQueueMode(remote.queue_mode);
      if (queued !== null) {
        setQueueMode(queued);
      }
      const thinking = reconcileThinkingMode(remote.thinking_mode);
      if (thinking !== null) {
        setThinkingMode(thinking);
      }
      const marks = reconcileHideMarks(remote.hide_marks);
      if (marks !== null) {
        setHideMarks(marks);
      }
      const suggest = reconcileSuggestPrompts(remote.suggest_prompts);
      if (suggest !== null) {
        setSuggestPrompts(suggest);
      }
      const notify = reconcileNotifications(remote.notifications);
      if (notify !== null) {
        setNotifications(notify);
      }
      const dev = reconcileDevtools(remote.devtools);
      if (dev !== null) {
        setDevtools(dev);
      }
    });
    const unsubscribeFont = tabs.onFontSizeChange((size) => persist(fontPatch(size)));
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
        dispatchTranscript({ type: 'conversation_loaded', messages: loaded.messages, todos: loaded.todos, context: loaded.context });
        setConversationId(loaded.id);
      } catch {
        if (transcriptRef.current.items.length === 0) {
          api.patchUiState({ last_conversation_id: null }).catch(() => undefined);
        }
      }
    };

    // The chat's events: the agent's, and the errors (see onMessage). The hold of the terminal is fed the agent's events too.
    const handleEvent = (ev: WireEvent) => {
      if (isSessionEvent(ev)) {
        // The terminal's own events come through handleTabEvent (see the controller); none should arrive here.
        return;
      }
      agentTarget(ev)?.applyAgentEvent(ev);
      switch (ev.type) {
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
          // Reconcile with the status: a lost finish event leaves a stale command id holding the border on,
          // but at a local idle prompt no agent command can still be running.
          {
            const target = agentTarget(ev);
            if (target !== null) {
              refreshControl(target);
            }
          }
          return;
        case 'steer_injected':
          // The turn took these: their rows leave the queue, and the transcript shows them at that step (transcript.ts).
          commitQueue(removeInjected(queueRef.current, ev.ids));
          dispatchTranscript({ type: 'agent_event', event: ev, at: Date.now() });
          return;
        case 'steer_returned':
          // The turn did not take these: they are plain queued messages again, sent at the end of the turn.
          commitQueue(unsteer(queueRef.current, ev.ids));
          return;
        default:
          dispatchTranscript({ type: 'agent_event', event: ev, at: Date.now() });
      }
    };

    // The terminal's own events. Their state is in the controller (see tabState.ts); these are what the transcript and the
    // agent's question also show.
    const handleTabEvent = (source: TabController, ev: SessionEvent) => {
      // Only the chat's own terminals reach its transcript and its question: an unassigned one's events stay out of both.
      if (!isOwnedByChat(source.getSnapshot().owner, CHAT_ID)) {
        return;
      }
      switch (ev.type) {
        case 'state':
          // A shell that exited ends a question still waiting (see agentRequestAfter).
          setAgentReq((cur) => agentRequestAfter(cur, ev));
          return;
        case 'levels':
          // The depth ends an interactive group a level was pushed above (see transcript.ts). The same for an integration the
          // agent asked for and one the user started. The approve-all state follows the new top level.
          dispatchTranscript({ type: 'levels_changed', depth: ev.levels.length });
          refreshAutoState();
          return;
        case 'record_finished':
          // Closes the interactive group the command opened on, if there is one (see transcript.ts).
          dispatchTranscript({ type: 'record_finished', record: ev.record });
          return;
        default:
          return;
      }
    };

    const onMessage = (msg: Incoming) => {
      if (msg.kind === 'error') {
        dispatchTranscript({ type: 'notice', text: msg.message });
      } else {
        handleEvent(msg.event);
      }
    };

    const unsubscribe = tabs.subscribeChat(onMessage);
    const unsubscribeTabs = tabs.subscribeTabEvents(handleTabEvent);
    // Reads the backend's terminals and connects them (see TabManager.start). The status is the active terminal's.
    tabs
      .start()
      .then((status) => {
        const active = tabs.active;
        if (status !== null && active !== null) {
          applyStatus(status, active);
          void restoreLast(status);
        }
      })
      .then(() => tabs.active?.fitToContainer())
      .catch(reportError);
    return () => {
      unsubscribe();
      unsubscribeTabs();
    };
  }, [applyStatus, refreshAutoState, refreshControl]);

  // The active terminal changed (a switch, a new one, a close). The new one is fitted to its host; it takes the keys unless
  // the focus is in the tab strip, where the arrow keys move it.
  useEffect(() => {
    if (!document.activeElement?.closest('.term-tabs')) {
      tab.focus();
    }
    void tab.fitToContainer();
  }, [tab]);

  // Focus follows the shell's end, not its commands (see focusAfter): the bar takes it when the shell exits, the
  // terminal when a new one starts.
  useEffect(() => {
    const target = focusAfter(prevEffective.current, effective);
    prevEffective.current = effective;
    if (target === 'xterm') {
      tab.focus();
    } else if (target === 'bar') {
      setFocusToken((t) => t + 1);
    }
  }, [effective]);

  // The panel closes while a full-screen program runs and opens again when it ends (see altPanel.ts). A user action in
  // that run goes through userPanel instead, and takes control of the panel.
  useEffect(() => {
    const step = altPanelStep(altPanel.current, { type: 'state', state: effective, open: layoutRef.current.open });
    altPanel.current = step.state;
    if (step.open !== null) {
      // Focus inside the panel would fall to <body> when it hides, and the full-screen program would lose the keys.
      if (step.open === false && panelRef.current?.contains(document.activeElement)) tab.focus();
      dispatchPanel({ type: 'open', open: step.open });
    }
  }, [effective]);

  // Saved on every change. A drag saves on each move; the write is a few hundred bytes. The UI state file gets the same
  // change, debounced (see uiSaver). While the panel is hidden for a full-screen program, the saved open state is the user's
  // (see layoutToSave), so the hiding is never saved as their choice.
  useEffect(() => {
    const saved = layoutToSave(panel, altPanel.current);
    savePanelLayout(saved, defaultSizeStorage());
    persist(panelPatch(saved));
  }, [panel]);

  // Refits the terminal once a dock change, a rail change or a drag's end is on screen. The fit reads the settled size
  // of .term-host. The ResizeObserver on .term-host refits too (with its own debounce); this one makes the final fit
  // land on the final layout. Runs after React has committed the change, so it never measures the old layout.
  useEffect(() => {
    const frame = requestAnimationFrame(() => void tab.fitToContainer());
    return () => cancelAnimationFrame(frame);
  }, [panel.dock, panel.open, settle]);

  // Ctrl+Shift+A (Cmd+Shift+A) focuses the agent bar, from anywhere it is offered. The app hotkeys (find, clear, zoom,
  // settings; see appHotkeys.ts for the Cmd keys on macOS and the Ctrl keys elsewhere) are handled here too. Capture
  // phase, so xterm does not also get the key, and a handled key is stopped here so it never reaches the PTY.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // While the AI settings are open they own the keyboard: only their own hotkey is taken here, so no shortcut reaches
      // the terminal or moves focus out of the dialog.
      if (settingsOpenRef.current) {
        const hotkey = appHotkeyFor(e);
        if (hotkey === 'settings') {
          e.preventDefault();
          e.stopPropagation();
        } else if (hotkey === 'find' || hotkey === 'find-terminal') {
          // No find while the settings are open, but the browser's own find never shows.
          e.preventDefault();
        }
        return;
      }
      if (isFocusBarHotkey(e)) {
        e.preventDefault();
        e.stopPropagation();
        // A side panel collapsed to the rail opens first. Both state changes land in one render, so the input is
        // visible when the focus effect in InputBar runs.
        const { open } = layoutRef.current;
        if (!open) {
          userPanel({ type: 'open', open: true });
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
      // Ctrl+A (Cmd+A) outside a field selects the focus zone's text, never the whole page. The terminal keeps its key.
      if (isSelectAllKey(e)) {
        const el = e.target instanceof Element ? e.target : null;
        const zone = selectAllTarget({
          editable: isEditableElement(el),
          inTerminal: el?.closest('.xterm') != null,
          inAgentPanel: el?.closest('.agent-panel') != null,
        });
        if (zone !== null) {
          e.preventDefault();
          if (zone === 'terminal') {
            tabs.active?.term.selectAll();
          } else {
            selectContentsOf(document.querySelector('.transcript-body'));
          }
          return;
        }
      }
      // In the terminal, off macOS, plain Ctrl+F is the shell's (see appHotkeys.ts); Ctrl+Shift+F still finds.
      const inTerminal = e.target instanceof Element && e.target.closest('.xterm') != null;
      const app = appHotkeyFor(e, undefined, inTerminal);
      if (app === null) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      switch (app) {
        case 'find': {
          // Cmd+F / Ctrl+F finds in the zone that has focus. Each zone's find is opened or, when open, its input is focused.
          const active = document.activeElement;
          if (findZone({ inAgentPanel: active?.closest('.agent-panel') != null }) === 'chat') {
            if (!active?.closest('.chat-find')) {
              chatFindOrigin.current = active;
            }
            setChatFindOpen(true);
            setChatFindToken((t) => t + 1);
          } else {
            setFindOpen(true);
            setFindToken((t) => t + 1);
          }
          return;
        }
        case 'find-terminal':
          setFindOpen(true);
          setFindToken((t) => t + 1);
          return;
        case 'clear':
          // Clears the scrollback locally; at the prompt Ctrl-L goes through the same route as typed keys (see clear.ts).
          tabs.active?.clear(effectiveRef.current, handleTerminalInput);
          return;
        case 'zoom-in':
          tabs.zoomFont('in');
          return;
        case 'zoom-out':
          tabs.zoomFont('out');
          return;
        case 'zoom-reset':
          tabs.zoomFont('reset');
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

  // Only the active terminal calls this (see TerminalView), so the keys go to the one on screen.
  const handleTerminalInput = useCallback((data: string) => {
    const id = tabs.active?.id;
    if (id !== undefined && routeRef.current.keys === 'pty') {
      api.writeUser(id, data).catch(reportError);
    }
  }, []);

  const closeFind = () => {
    setFindOpen(false);
    tab.focus();
  };

  // Esc in the chat find: closes it and gives focus back to where it was opened from. The terminal find is not touched.
  const closeChatFind = () => {
    setChatFindOpen(false);
    const origin = chatFindOrigin.current;
    chatFindOrigin.current = null;
    if (origin instanceof HTMLElement && origin.isConnected) {
      origin.focus();
    }
  };

  // The context menu's find items: the same as Ctrl+Shift+F, and Ctrl+F in the agent panel (see the keydown listener).
  const openTerminalFind = () => {
    setFindOpen(true);
    setFindToken((t) => t + 1);
  };

  const openChatFind = () => {
    const active = document.activeElement;
    if (!active?.closest('.chat-find')) {
      chatFindOrigin.current = active;
    }
    setChatFindOpen(true);
    setChatFindToken((t) => t + 1);
  };

  const sendRaw = (text: string) => {
    api.writeUser(tab.id, text).catch(reportError);
  };

  const sendAgent = async (text: string, attachments: AttachmentInput[]): Promise<string | null> => {
    // Refused while a switch is in flight: the text stays in the bar, and nothing lands in the conversation being switched to.
    if (switching.current) {
      return SEND_WHILE_SWITCHING_NOTICE;
    }
    dropSuggestions();
    // A message is a new turn: what was saved for the reply before it no longer follows the chat.
    forgetLiveSuggestions();
    epoch.current += 1;
    clientIds.current += 1;
    const clientId = clientIds.current;
    // Added before the call, so the user's line lands above the agent's replies.
    dispatchTranscript({ type: 'user_sent', clientId, text, attachments: userAttachments(attachments) });
    try {
      await api.agentMessage(text, attachments);
      return null;
    } catch (e) {
      const message = errorText(e);
      dispatchTranscript({ type: 'user_failed', clientId, message });
      return message;
    }
  };

  /** Replaces the queue: the state for the render, and the ref for the steps that run outside it. */
  const commitQueue = (next: QueuedMessage[]) => {
    queueRef.current = next;
    setQueue(next);
  };

  /** The suggestions no longer follow the reply: they are cleared, and a request in flight is stale (its result is dropped). */
  const dropSuggestions = () => {
    suggestGen.current += 1;
    suggestAgain.current = false;
    setSuggestions([]);
  };

  /** The live conversation's saved suggestions are removed: a turn started, so they no longer follow the chat. */
  const forgetLiveSuggestions = () => {
    if (conversationRef.current !== null) {
      forgetSuggestions(cacheStorage, conversationRef.current);
    }
  };

  /** The conversation changed: what is queued was for the old one, so it is dropped rather than sent into the new one. */
  const clearQueue = () => {
    queueGen.current += 1;
    commitQueue([]);
    dropSuggestions();
  };

  /**
   * Takes what is queued back into the input (a Stop, a restart): nothing is sent. InputBar restores it when the tick moves,
   * so the queue's messages come back as an unqueue does, with their files.
   */
  const returnQueued = () => {
    if (queueRef.current.length === 0) {
      return;
    }
    returned.current = [...returned.current, ...queueRef.current];
    commitQueue([]);
    setRestoreTick((t) => t + 1);
  };

  /** The messages returned for the input to restore (see returnQueued). Read once. */
  const takeReturned = (): QueuedMessage[] => {
    const items = returned.current;
    returned.current = [];
    return items;
  };

  /**
   * Asks the backend to cancel the running turn, then reads the status again. If the backend is already idle, the busy
   * state follows it (applyStatus), so a turn the UI thought was running cannot stay busy.
   */
  const cancelRunning = () => {
    api.cancelAgent().catch(reportError).then(() => refreshStatus());
  };

  /**
   * Waits until the backend's turn slot is free. `done` is emitted before the slot is released (see refreshControl), so a
   * send right after it can be refused. Polls the status; after QUEUE_SLOT_WAIT_MS it gives up waiting, and the send goes
   * anyway (a refusal then puts the messages back in the queue).
   */
  const waitForSlot = async (): Promise<void> => {
    const deadline = Date.now() + QUEUE_SLOT_WAIT_MS;
    for (;;) {
      try {
        const status = await api.getStatus();
        if (!status.agent_busy) {
          return;
        }
      } catch {
        return;
      }
      if (Date.now() >= deadline) {
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, QUEUE_SLOT_POLL_MS));
    }
  };

  /**
   * Sends what is queued, once a turn has ended: all of it as one message, or the first message in `one` mode. The
   * queue is read after the slot wait, so a message removed meanwhile is not sent. A refused send puts the messages back
   * at the front of the queue (no retry; the user sees the queue and the error in the transcript). One drain at a time.
   */
  const drainQueue = async () => {
    // While a conversation is being switched, the queue is left alone: the switch clears it (clearQueue) when it lands.
    if (draining.current || switching.current) {
      return;
    }
    draining.current = true;
    const gen = queueGen.current;
    try {
      await waitForSlot();
      if (queueGen.current !== gen) {
        return;
      }
      const pending = queueRef.current;
      const { send, rest } = drainBatch(pending, queueMode);
      if (send === null) {
        return;
      }
      const taken = pending.slice(0, pending.length - rest.length);
      commitQueue(rest);
      const err = await sendAgent(send.text, send.attachments);
      if (err !== null && queueGen.current === gen) {
        commitQueue([...taken, ...queueRef.current]);
      }
    } finally {
      draining.current = false;
      // A turn ended while this drain was running (its `done` can come before the send returns): the edge was not drained then.
      if (drainAgain.current) {
        drainAgain.current = false;
        if (queueRef.current.length > 0) {
          void drainQueue();
        }
      }
    }
  };

  /**
   * Asks for follow-up prompts for the reply that just ended. Read-only on the backend: it never touches busy, the
   * transcript or the queue. Errors are dropped silently, so a failed request shows nothing. One at a time: a request
   * asked for while one runs is run once more after it, unless the agent is busy or something is queued by then.
   */
  const requestSuggestions = async (): Promise<void> => {
    if (suggesting.current) {
      suggestAgain.current = true;
      return;
    }
    suggesting.current = true;
    const gen = suggestGen.current;
    const conversation = conversationRef.current;
    try {
      const result = await api.suggestPrompts();
      const shown = acceptSuggestions(result, { current: suggestGen.current === gen, busy: isBusy(transcriptRef.current) });
      setSuggestions(shown);
      // Kept under the conversation they were asked in, for its tail as it is now (see conversationCache.ts). A result shown
      // only when that conversation is still the live one.
      if (shown.length > 0 && conversation !== null && conversationRef.current === conversation) {
        saveSuggestions(cacheStorage, conversation, shown, transcriptSig(transcriptRef.current.items));
      }
    } catch {
      // Nothing to show: the input bar keeps its plain look.
    } finally {
      suggesting.current = false;
      if (suggestAgain.current) {
        suggestAgain.current = false;
        if (!isBusy(transcriptRef.current) && queueRef.current.length === 0) {
          void requestSuggestions();
        }
      }
    }
  };

  /** A suggested prompt was clicked: it is sent as a plain message, the same as a typed one but not through the command parser. */
  const sendSuggestion = (text: string) => {
    void sendAgent(text, []);
  };

  /**
   * Offers a queued message to the running turn (steer_message): it goes in at the agent's next step. Accepted: the row is
   * marked steered. Refused or failed: the row stays a plain queued message, sent at the end of the turn as before (a
   * failure is not shown here; a send that really fails shows its error when it is sent). The row may be gone by the
   * answer (injected, withdrawn, restored by Stop): markSteered then changes nothing.
   */
  const steerQueued = (id: number, text: string, attachments: AttachmentInput[]) => {
    api.steerMessage(id, text, attachments, queueMode).then(
      (accepted) => {
        if (accepted) {
          commitQueue(markSteered(queueRef.current, id));
        }
      },
      () => undefined,
    );
  };

  /**
   * Takes a steered message back (withdraw_steer). The turn decides: if it was still waiting, the row goes and its text
   * comes back into the input, as an unqueue does. If the turn already took it, nothing is done here: steer_injected
   * removes the row. A failed call leaves the row as it is.
   */
  const withdrawSteered = async (id: number): Promise<void> => {
    const waiting = await api.withdrawSteer(id).catch((e: unknown) => {
      reportError(e);
      return null;
    });
    if (waiting === null) {
      return;
    }
    const { queue: next, removed } = takeWithdrawn(queueRef.current, id, waiting);
    if (removed !== null) {
      commitQueue(next);
      // Restored the way a Stop restores the queue: the input takes it on the tick (see InputBar's restoreTick).
      returned.current = [...returned.current, removed];
      setRestoreTick((t) => t + 1);
    }
  };

  /** Takes a queued message back, for the input to restore. Null when it is no longer queued, or when it is steered (withdrawSteered asks the turn first). */
  const unqueue = (id: number): QueuedMessage | null => {
    if (queueRef.current.some((q) => q.id === id && q.steered === true)) {
      void withdrawSteered(id);
      return null;
    }
    const { queue: next, removed } = removeQueued(queueRef.current, id);
    if (removed !== null) {
      commitQueue(next);
    }
    return removed;
  };

  /** The queue mode is applied at once and saved with the UI state. */
  const setQueueModeSetting = (mode: QueueMode) => {
    setQueueMode(mode);
    persist(queueModePatch(mode));
  };

  /** The reasoning setting is applied at once and saved with the UI state. */
  const setThinkingModeSetting = (mode: ThinkingMode) => {
    setThinkingMode(mode);
    persist(thinkingModePatch(mode));
  };

  /** The suggest-prompts setting is applied at once and saved. Turning it off drops what is shown or in flight, and what was saved. */
  const setSuggestPromptsSetting = (on: boolean) => {
    dropSuggestions();
    if (!on) {
      forgetAllSuggestions(cacheStorage);
    }
    setSuggestPrompts(on);
    persist(suggestPromptsPatch(on));
  };

  /** The notifications setting is applied at once and saved with the UI state. */
  const setNotificationsSetting = (on: boolean) => {
    setNotifications(on);
    persist(notificationsPatch(on));
  };

  /** The developer (layout inspector) setting is applied at once and saved with the UI state. */
  const setDevtoolsSetting = (on: boolean) => {
    setDevtools(on);
    persist(devtoolsPatch(on));
  };

  // Clears the agent's conversation. Refused while the agent is busy or a side command runs (the button is also disabled then),
  // before the backend is asked. On error the reason is shown in the transcript, and the conversation stays as it was.
  const newConversation = () => {
    const refusal = switchRefusal({ busy: isBusy(transcriptRef.current), sideBusy, switching: switching.current });
    if (refusal !== null) {
      dispatchTranscript({ type: 'notice', text: refusal });
      return;
    }
    switching.current = true;
    epoch.current += 1;
    api.newConversation()
      .then(
        () => {
          clearQueue();
          dispatchTranscript({ type: 'conversation_reset' });
          // The new conversation has a new id, which the history view marks as current. Read before the switch ends.
          return refreshStatus();
        },
        (e) => dispatchTranscript({ type: 'notice', text: errorText(e) }),
      )
      .finally(() => {
        switching.current = false;
      });
  };

  /** Opens the agent panel on the chat view and moves focus to the bar. */
  const showChat = () => {
    userPanel({ type: 'open', open: true });
    setPanelBody('chat');
    setFocusToken((t) => t + 1);
  };

  /** The `/history` command. It opens the panel if collapsed and shows History; unlike the button it never toggles it closed. */
  const showHistory = () => {
    if (!panel.open) {
      userPanel({ type: 'open', open: true });
    }
    // Remount, so the search field takes focus even when History is already shown.
    setHistoryKey((k) => k + 1);
    setPanelBody('history');
  };

  /**
   * Switches to a conversation the backend already adopted (`/compact`, `/fork`): the loaded messages, todos and
   * context become the live transcript at once. Never `openConversation`: that would reload from the store and race
   * a send, while this result is already the backend's live state.
   */
  const applyLoaded = (loaded: LoadedConversation) => {
    epoch.current += 1;
    clearQueue();
    dispatchTranscript({ type: 'conversation_loaded', messages: loaded.messages, todos: loaded.todos, context: loaded.context });
    setConversationId(loaded.id);
    setPanelBody('chat');
    refreshStatus();
  };

  /**
   * Runs a parsed slash command. `handled` means a local action ran (nothing is sent); `send` means the expanded
   * text goes to the agent as a normal user message; `unknown` means the name is not a command, so the original
   * text goes to the agent. Built-ins win over custom commands of the same name.
   */
  const runCommand = async (
    name: string,
    args: string,
    original: string,
  ): Promise<{ kind: 'send'; text: string } | { kind: 'handled' } | { kind: 'unknown' }> => {
    const builtin = findBuiltin(name);
    // Not a built-in: fetch the catalog fresh (the cwd can change) and expand the template.
    if (builtin === null) {
      let catalog;
      try {
        catalog = await api.listCommands(tab.id);
      } catch {
        // The lookup failed: send the original text rather than swallow it.
        return { kind: 'send', text: original };
      }
      const cmd = catalog.commands.find((c) => c.name === name);
      if (cmd === undefined) {
        return { kind: 'unknown' };
      }
      return { kind: 'send', text: expandTemplate(cmd.template, args) };
    }
    switch (builtin.name) {
      case 'help': {
        loadHelp();
        setHelpOpen(true);
        return { kind: 'handled' };
      }
      case 'new': {
        newConversation();
        return { kind: 'handled' };
      }
      case 'stop': {
        // A side command holds no shell command: cancel it without touching the terminal.
        if (sideBusy) {
          cancelRunning();
          return { kind: 'handled' };
        }
        if (!isBusy(transcriptRef.current)) {
          dispatchTranscript({ type: 'notice', text: 'No turn is running.', muted: true });
          return { kind: 'handled' };
        }
        cancelRunning();
        // Queued messages come back into the input, not sent. This runs after the bar clears the typed "/stop" (it is still
        // in the bar until this returns), so the restored text is not lost with it.
        setTimeout(returnQueued, 0);
        // ^C only when the agent holds the shell, so stopping a thinking/waiting turn never kills the
        // user's own foreground process.
        if (!locked && borderActive(agentControl)) {
          sendRaw(CTRL_C);
        }
        return { kind: 'handled' };
      }
      case 'model': {
        // Bare `/model` opens Settings; with an argument the default model is switched.
        if (args === '') {
          openSettingsForModel();
          return { kind: 'handled' };
        }
        try {
          const view = await api.getSettings();
          const options: ModelChoice[] = view.settings.providers.flatMap((p) =>
            p.models.map((m) => ({
              ref: { provider_id: p.id, model_id: m.id },
              label: `${p.name} · ${m.id}`,
              provider: p.name,
              variants: effectiveVariants(m).map((v) => ({ id: v.id, label: v.label })),
            })),
          );
          const matched = matchModel(options, args);
          if (matched.status === 'match') {
            await api.setDefaultModel(matched.choice.ref);
            refreshStatus();
            refreshSettingsState();
            const variant = matched.choice.ref.variant ?? null;
            dispatchTranscript({
              type: 'notice',
              text: `Default model: ${matched.choice.label}${variant ? `#${variant}` : ''}.`,
              muted: true,
            });
          } else if (matched.status === 'ambiguous') {
            const list = matched.candidates
              .slice(0, 8)
              .map((c) => `${c.ref.provider_id}/${c.ref.model_id}${c.ref.variant ? `#${c.ref.variant}` : ''}`)
              .join(', ');
            dispatchTranscript({ type: 'notice', text: `Multiple models match "${args}": ${list}.`, muted: true });
          } else if (matched.variants.length > 0) {
            const { base, variant } = splitModelVariant(args);
            dispatchTranscript({
              type: 'notice',
              text: `No variant "${variant ?? ''}" on "${base}". Available: ${matched.variants.slice(0, 8).join(', ')}.`,
              muted: true,
            });
          } else {
            const list = options
              .slice(0, 8)
              .map((c) => `${c.ref.provider_id}/${c.ref.model_id}`)
              .join(', ');
            dispatchTranscript({
              type: 'notice',
              text: options.length === 0 ? `No model matches "${args}".` : `No model matches "${args}". Available: ${list}.`,
              muted: true,
            });
          }
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        }
        return { kind: 'handled' };
      }
      case 'export': {
        try {
          const md = await api.exportConversation();
          if (md.trim() === '') {
            dispatchTranscript({ type: 'notice', text: 'There is nothing to export yet.', muted: true });
            return { kind: 'handled' };
          }
          const ok = await copyText(md, { clipboard: navigator.clipboard ?? undefined, document });
          dispatchTranscript({
            type: 'notice',
            text: ok
              ? `Copied the conversation to the clipboard (${md.length} characters).`
              : 'Could not copy the conversation to the clipboard.',
            ...(ok ? { muted: true } : {}),
          });
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        }
        return { kind: 'handled' };
      }
      case 'plan':
        return { kind: 'send', text: expandTemplate(PLAN_TEMPLATE, args) };
      case 'review':
        return { kind: 'send', text: expandTemplate(REVIEW_TEMPLATE, args) };
      case 'init':
        return { kind: 'send', text: expandTemplate(INIT_TEMPLATE, args) };
      case 'context': {
        dispatchTranscript({ type: 'notice', text: renderContext(transcriptRef.current.context), muted: true });
        return { kind: 'handled' };
      }
      case 'history': {
        showHistory();
        return { kind: 'handled' };
      }
      case 'rename': {
        // With no title the agent suggests one: a side call outside a turn, never stored. Only the notices below
        // reach the transcript.
        if (args === '') {
          if (conversationId === null) {
            dispatchTranscript({ type: 'notice', text: 'There is no conversation to rename yet.', muted: true });
            return { kind: 'handled' };
          }
          if (sideBusy) {
            dispatchTranscript({ type: 'notice', text: 'A side command is still running. Wait for it to finish, or stop it first.', muted: true });
            return { kind: 'handled' };
          }
          if (isBusy(transcriptRef.current)) {
            dispatchTranscript({ type: 'notice', text: 'The agent is busy. Wait for the turn to finish, or stop it first.', muted: true });
            return { kind: 'handled' };
          }
          setSideBusy(true);
          dispatchTranscript({ type: 'notice', text: 'Naming the conversation...', muted: true });
          try {
            const name = await api.suggestName();
            const meta = await api.renameConversation(conversationId, name);
            setHistoryKey((k) => k + 1);
            dispatchTranscript({ type: 'notice', text: `Renamed the conversation to "${meta.title}".`, muted: true });
          } catch (e) {
            dispatchTranscript({ type: 'notice', text: errorText(e) });
          } finally {
            setSideBusy(false);
          }
          return { kind: 'handled' };
        }
        if (conversationId === null) {
          dispatchTranscript({ type: 'notice', text: 'There is no conversation to rename yet.', muted: true });
          return { kind: 'handled' };
        }
        try {
          const meta = await api.renameConversation(conversationId, args);
          setHistoryKey((k) => k + 1);
          dispatchTranscript({ type: 'notice', text: `Renamed the conversation to "${meta.title}".`, muted: true });
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        }
        return { kind: 'handled' };
      }
      case 'btw': {
        // A side question: one model call outside a turn. The answer is only a muted notice, never stored.
        if (sideBusy) {
          dispatchTranscript({ type: 'notice', text: 'A side command is still running. Wait for it to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (isBusy(transcriptRef.current)) {
          dispatchTranscript({ type: 'notice', text: 'The agent is busy. Wait for the turn to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (args === '') {
          dispatchTranscript({ type: 'notice', text: 'Usage: /btw <question>.', muted: true });
          return { kind: 'handled' };
        }
        setSideBusy(true);
        try {
          const answer = await api.askBtw(args);
          dispatchTranscript({ type: 'notice', text: `Side note:\n${answer}`, muted: true });
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        } finally {
          setSideBusy(false);
        }
        return { kind: 'handled' };
      }
      case 'compact': {
        // Summarize the conversation into a new one and switch to it atomically. The old one stays in history.
        if (sideBusy) {
          dispatchTranscript({ type: 'notice', text: 'A side command is still running. Wait for it to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (isBusy(transcriptRef.current)) {
          dispatchTranscript({ type: 'notice', text: 'The agent is busy. Wait for the turn to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (args !== '') {
          dispatchTranscript({ type: 'notice', text: 'Usage: /compact takes no arguments.', muted: true });
          return { kind: 'handled' };
        }
        setSideBusy(true);
        dispatchTranscript({ type: 'notice', text: 'Compacting the conversation...', muted: true });
        try {
          applyLoaded(await api.compactConversation());
          dispatchTranscript({ type: 'notice', text: 'Compacted into a new conversation.', muted: true });
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        } finally {
          setSideBusy(false);
        }
        return { kind: 'handled' };
      }
      case 'fork': {
        // Copy the conversation into a new one and switch to it atomically. The old one stays untouched.
        if (sideBusy) {
          dispatchTranscript({ type: 'notice', text: 'A side command is still running. Wait for it to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (isBusy(transcriptRef.current)) {
          dispatchTranscript({ type: 'notice', text: 'The agent is busy. Wait for the turn to finish, or stop it first.', muted: true });
          return { kind: 'handled' };
        }
        if (args !== '') {
          dispatchTranscript({ type: 'notice', text: 'Usage: /fork takes no arguments.', muted: true });
          return { kind: 'handled' };
        }
        setSideBusy(true);
        try {
          applyLoaded(await api.forkConversation());
          dispatchTranscript({ type: 'notice', text: 'Forked into a new conversation.', muted: true });
        } catch (e) {
          dispatchTranscript({ type: 'notice', text: errorText(e) });
        } finally {
          setSideBusy(false);
        }
        return { kind: 'handled' };
      }
      default:
        return { kind: 'unknown' };
    }
  };

  /**
   * The bar's send: while a side command runs, every send is refused with an error so the InputBar keeps the text.
   * Otherwise a slash command runs locally or expands into a user message, anything else (including unknown
   * `/...`) is sent as it was typed. Template commands send the expanded text, so that is what the transcript and
   * the stored history hold.
   */
  const onAgent = async (text: string, attachments: AttachmentInput[]): Promise<string | null> => {
    if (sideBusy) {
      return 'A side command is still running.';
    }
    // While a turn runs, a plain message (or an unknown `/...`, which is one) waits in the queue instead of being refused.
    // Read again after any await: the turn may have ended meanwhile.
    const queueIt = (queued: string): null => {
      queueSeq.current += 1;
      const id = queueSeq.current;
      commitQueue(enqueue(queueRef.current, id, queued, attachments));
      steerQueued(id, queued, attachments);
      return null;
    };
    const parsed = parseCommand(text);
    if (parsed === null) {
      return isBusy(transcriptRef.current) ? queueIt(text) : sendAgent(text, attachments);
    }
    const outcome = await runCommand(parsed.name, parsed.args, text);
    // A template expands into a message: queued as that message, like a plain one, while busy.
    if (outcome.kind === 'send') {
      return isBusy(transcriptRef.current) ? queueIt(outcome.text) : sendAgent(outcome.text, attachments);
    }
    if (outcome.kind === 'unknown') {
      return isBusy(transcriptRef.current) ? queueIt(text) : sendAgent(text, attachments);
    }
    return null;
  };

  /**
   * A saved conversation was opened from the History view: it becomes the live transcript, and the chat is shown with the
   * input focused. The backend refuses the load while the agent is busy. A send, a new conversation or another open that
   * happened while this one loaded is not overwritten: the result is refused, and the status is read again.
   * Resolves with the reason it was refused, or null when it was opened.
   */
  const openConversation = async (id: string): Promise<string | null> => {
    const refusal = switchRefusal({ busy: isBusy(transcriptRef.current), sideBusy, switching: switching.current });
    if (refusal !== null) {
      return refusal;
    }
    switching.current = true;
    try {
      const started = epoch.current;
      let loaded: LoadedConversation;
      try {
        loaded = await api.loadConversation(id);
      } catch (e) {
        return errorText(e);
      }
      if (epoch.current !== started || isBusy(transcriptRef.current)) {
        await syncConversationId();
        return 'The chat changed while this conversation was loading, so it was not opened. Open it again.';
      }
      epoch.current += 1;
      clearQueue();
      dispatchTranscript({ type: 'conversation_loaded', messages: loaded.messages, todos: loaded.todos, context: loaded.context });
      setConversationId(loaded.id);
      setPanelBody('chat');
      setFocusToken((t) => t + 1);
      await refreshStatus();
      return null;
    } finally {
      switching.current = false;
    }
  };

  /** The live conversation was deleted from the History view: the backend started a new one, so the chat is reset to match. */
  const liveDeleted = () => {
    epoch.current += 1;
    clearQueue();
    dispatchTranscript({ type: 'conversation_reset' });
    // Its unsent text is dropped with it (see InputBar's draftDiscard), not saved under the deleted id.
    setDiscardDraft((d) => d + 1);
    setConversationId(null);
    refreshStatus();
  };

  /** A conversation was deleted from the History view (or all of them). What was kept for it goes too. */
  const conversationDeleted = (id: string | 'all') => {
    if (id === 'all') {
      forgetEverything(cacheStorage);
    } else {
      forgetConversation(cacheStorage, id);
    }
  };

  /** Moves focus to the History header button, so it is not lost when the view closes. */
  const focusHistoryButton = () => document.getElementById('history-button')?.focus();

  /** The History button. It opens the panel first if it is collapsed, so the view is visible. Closing returns focus to it. */
  const toggleHistory = () => {
    if (!panel.open) {
      userPanel({ type: 'open', open: true });
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

  const setMarksHidden = (hide: boolean) => {
    setHideMarks(hide);
    persist(hideMarksPatch(hide));
  };

  const answer = (callId: string, approve: boolean) => {
    dispatchTranscript({ type: 'approval_answering', callId, approve });
    api.approveTool(callId, approve).then(
      () => dispatchTranscript({ type: 'approval_sent', callId }),
      (e) => dispatchTranscript({ type: 'approval_failed', callId, message: errorText(e) }),
    );
  };

  /**
   * Answers the agent's clarification question. The answers go back through `answer_questions` only: they never reach
   * the shell or the approval paths. A refused send fails the card, so the user can retry it.
   */
  const answerQuestions = (callId: string, answers: QuestionAnswerInput[]) => {
    dispatchTranscript({ type: 'question_answering', callId, answers });
    api.answerQuestions(callId, answers).catch((e) => {
      dispatchTranscript({ type: 'question_failed', callId, message: errorText(e) });
    });
  };

  const pendingApproval = firstPendingApproval(transcript);
  // A question waiting for the user's answer: a focus blocker for the input bar (see inputFocus.ts).
  const questionPending = transcript.items.some((i) => i.kind === 'question' && i.state === 'pending');
  // The waiting card itself, for the toast while the chat body is hidden. Null when no card waits.
  const pendingApprovalItem =
    pendingApproval !== null
      ? (transcript.items.find(
          (i): i is Extract<TranscriptItem, { kind: 'approval' }> => i.kind === 'approval' && i.callId === pendingApproval,
        ) ?? null)
      : null;
  // Enter approves only once the card has been on screen a beat and the chat body is visible: a fresh card,
  // or a hidden/history-view card, cannot be approved by Enter. Escape still denies on the pending card alone.
  const [approvalArmed, setApprovalArmed] = useState(false);
  useEffect(() => {
    if (pendingApproval === null) {
      setApprovalArmed(false);
      return;
    }
    setApprovalArmed(false);
    const timer = setTimeout(() => setApprovalArmed(true), 500);
    return () => clearTimeout(timer);
  }, [pendingApproval]);
  const canApprove = approvalArmed && chatBodyShown(panel.open, panelBody);
  const busy = isBusy(transcript);
  // A turn or side command that has just ended sends what is queued (see drainQueue). The ref is the previous render's value.
  const active = busy || sideBusy;
  const wasActive = useRef(active);
  useEffect(() => {
    const prev = wasActive.current;
    wasActive.current = active;
    if (shouldDrain(prev, active, queueRef.current.length)) {
      if (draining.current) {
        drainAgain.current = true;
      } else {
        void drainQueue();
      }
    }
    // The same edge asks for follow-up prompts (see suggestions.ts). A turn starting clears what was shown, and what was saved.
    if (active) {
      dropSuggestions();
      forgetLiveSuggestions();
    } else if (
      shouldSuggest({
        enabled: suggestPrompts,
        turnJustEnded: prev,
        queueLength: queueRef.current.length,
        sideBusy,
        lastItemIsAgentText: endsWithAgentText(transcript.items),
      })
    ) {
      void requestSuggestions();
    }
  }, [active]);
  // The follow-up prompts saved for the live conversation come back when its transcript still ends with the reply they followed
  // (the signature matches), the setting is on, and nothing is running. Read on open, on start, and when the setting turns on.
  // A saved entry for another tail is stale: it is removed, once there is a transcript to compare with.
  useEffect(() => {
    if (!suggestPrompts || active || conversationId === null || suggestions.length > 0) {
      return;
    }
    const saved = loadSuggestions(cacheStorage, conversationId, transcriptSig(transcript.items));
    if (saved === 'stale') {
      if (transcript.items.length > 0) {
        forgetSuggestions(cacheStorage, conversationId);
      }
      return;
    }
    if (saved !== null) {
      setSuggestions(saved.items);
    }
  }, [conversationId, suggestPrompts, active, transcript.items]);
  // Desktop notifications (see attention.ts): sent with the current setting and focus. Failures are handled in attentionClient.
  const notifyAttention = (event: AttentionEvent) => {
    const decision = decideAttention(event, {
      enabled: notifications,
      focused: windowFocused.current,
      queueLength: queueRef.current.length,
    });
    if (decision !== null) {
      void sendAttention(decision);
    }
  };
  /** True the first time a key is seen, so each question or approval is notified once. */
  const firstAttention = (key: string): boolean => {
    if (attentionKey.current === key) {
      return false;
    }
    attentionKey.current = key;
    return true;
  };
  // The window's focus, and a click on a notification (it brings the window forward). Registered once. When a listener
  // cannot be set up, the defaults stay: focused, and a click does nothing.
  useEffect(() => {
    const stops: Array<() => void> = [];
    let gone = false;
    const keep = (stop: () => void) => {
      if (gone) {
        stop();
      } else {
        stops.push(stop);
      }
    };
    watchWindowFocus((focused) => {
      windowFocused.current = focused;
    }).then(keep, () => undefined);
    onNotificationClick(() => {
      api.focusMainWindow().catch(() => undefined);
    }).then(keep, () => undefined);
    return () => {
      gone = true;
      stops.forEach((stop) => stop());
    };
  }, []);
  // The turn ended (the busy edge). Not for a side command. A queued message is about to be sent, so it is not idle.
  const wasBusy = useRef(busy);
  useEffect(() => {
    const prev = wasBusy.current;
    wasBusy.current = busy;
    if (prev && !busy) {
      const last = transcript.items[transcript.items.length - 1];
      notifyAttention({ type: 'turn_idle', lastText: last !== undefined && last.kind === 'agent' ? last.text : null });
    }
  }, [busy]);
  // A question or an approval waiting for the user. Each card notifies once, keyed by its call id.
  const pendingQuestionItem =
    transcript.items.find(
      (i): i is Extract<TranscriptItem, { kind: 'question' }> => i.kind === 'question' && i.state === 'pending',
    ) ?? null;
  useEffect(() => {
    if (pendingQuestionItem === null || !firstAttention(`question:${pendingQuestionItem.callId}`)) {
      return;
    }
    notifyAttention({ type: 'question', text: pendingQuestionItem.questions[0]?.question ?? '' });
  }, [pendingQuestionItem?.callId]);
  useEffect(() => {
    if (pendingApprovalItem === null || !firstAttention(`approval:${pendingApprovalItem.callId}`)) {
      return;
    }
    const shownInstead = showsDisplay(pendingApprovalItem.name, pendingApprovalItem.reason);
    const text = shownInstead ? (pendingApprovalItem.display ?? pendingApprovalItem.text) : pendingApprovalItem.text;
    notifyAttention({ type: 'approval', text });
  }, [pendingApprovalItem?.callId]);
  // Same busy as the input bar's loader (busy || sideBusy). An exited shell takes the border off even if a turn is still open.
  const border: BorderLevel = borderLevel(agentControl, (busy || sideBusy) && effective !== 'exited');
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
    api.integrateShell(tab.id).then(
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
    tab.focus();
  };

  // Starts a new shell after the old one exited. The terminal is cleared first, so nothing from the old shell
  // shows up in the new one. Focus moves to the terminal when the new shell starts (see focusAfter).
  const restartShell = () => {
    if (restarting) {
      return;
    }
    tab.setRestarting(true);
    tab.resetForNewSession();
    // The chat's own terminal: the same conversation goes on, so what was queued comes back into the input rather than
    // being dropped. Another chat's or an unassigned terminal has none of that in this chat.
    if (isOwnedByChat(tab.getSnapshot().owner, CHAT_ID)) {
      returnQueued();
      clearQueue();
      dispatchTranscript({ type: 'session_reset' });
      setAgentReq((cur) => agentRequestClosedBySession(cur));
    }
    tab.resetAgentControl();
    tab
      .restart()
      .then(() => api.getStatus(tab.id))
      .then((s) => applyStatus(s, tab))
      .then(() => tab.fitToContainer())
      .catch(reportError)
      // Still true until the status is applied, so a second click cannot land while the old exit state is shown.
      .finally(() => tab.setRestarting(false));
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
    return el.getBoundingClientRect().width;
  };
  const activity = agentActivity(transcript.items, transcript.thinking, sideBusy);

  return (
    <ActiveTabContext.Provider value={tab}>
    <div className="app">
      <StatusChip
        integration={integration}
        observer={observer}
        input={strip.state}
        foreground={strip.foreground}
        cwd={cwd}
        location={levels.length > 0 ? levels[levels.length - 1] : null}
        canIntegrate={integrate.kind === 'closed' && agentReq === null && canIntegrate(strip.state, strip.foreground)}
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
            <TabStrip />
            <div className="term-stack">
              {tabs.list().map((t) => (
                <TerminalView
                  key={t.id}
                  controller={t}
                  active={t === tab}
                  keysToPty={keyRoute.keys === 'pty'}
                  inputState={effective}
                  onInput={handleTerminalInput}
                  onAltChange={(alt) => t.dispatchInput({ type: 'alt', active: alt })}
                />
              ))}
              {/* Shown during the agent's turn ('working', softer) and while it holds the shell ('control'). Never takes clicks; sits above the terminal background, below its decorations. */}
              <div className={borderClass[border]} aria-hidden="true">
                <div className="agent-control-glow" />
              </div>
              {findOpen && <FindBar focusToken={findToken} onClose={closeFind} />}
            </div>
          </div>
        }
        divider={
          view.divider ? (
            <PanelDivider
              dock={panel.dock}
              container={workspace}
              valueNow={panelBox.width}
              measure={measurePanel}
              onResize={(size) => dispatchPanel({ type: 'size', dock: panel.dock, size })}
              onReset={() => dispatchPanel({ type: 'reset_size', dock: panel.dock })}
              onSettle={settleLayout}
            />
          ) : null
        }
        panel={
          <AgentPanel
            ref={panelRef}
            view={view}
            onExpand={() => userPanel({ type: 'toggle' })}
            busy={busy || sideBusy}
            thinking={transcript.thinking}
            approvalPending={pendingApproval !== null}
          >
            {chatFindOpen && <ChatFindBar focusToken={chatFindToken} onClose={closeChatFind} />}
            <Transcript
              items={transcript.items}
              busy={busy}
              thinking={transcript.thinking}
              thinkingMode={thinkingMode}
              hasOlder={hasOlder(transcript)}
              onLoadOlder={() => dispatchTranscript({ type: 'older_loaded' })}
              approvalPending={pendingApproval !== null}
              open={panel.open}
              onToggle={() => userPanel({ type: 'toggle' })}
              onApprove={answer}
              onAnswerQuestions={answerQuestions}
              onNewConversation={newConversation}
              newBlocked={busy || sideBusy}
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
                    key={historyKey}
                    currentId={conversationId}
                    busy={busy || sideBusy}
                    groupBy={historyGroupBy}
                    onGroupBy={setGroupBy}
                    onOpen={openConversation}
                    onLiveDeleted={liveDeleted}
                    onDeleted={conversationDeleted}
                    onClose={closeHistory}
                  />
                ) : null
              }
            />
            {chatBodyShown(panel.open, panelBody) && <TodoPanel items={transcript.todos} />}
            <ContextMeter usage={transcript.context} />
            <InputBar
              mode={barMode}
              hint={locked ? 'Integrating this shell. Keys are not sent until it is done.' : route.hint}
              hintInline={locked ? false : route.hintInline}
              locked={locked}
              questionPending={questionPending}
              hasPendingApproval={pendingApproval !== null}
              canApprove={canApprove}
              busy={busy || sideBusy}
              focusToken={focusToken}
              onFocusChange={setBarOpen}
              loadCommands={() => api.listCommands(tab.id)}
              onRestart={restartShell}
              restarting={restarting}
              onAgent={onAgent}
              autoApprove={autoApprove}
              autoSuspended={autoSuspended}
              onToggleAutoApprove={toggleAutoApprove}
              modelVision={modelVision}
              queue={queue}
              queueMode={queueMode}
              queueable={busy && !sideBusy}
              onUnqueue={unqueue}
              suggestions={suggestions}
              onSuggestion={sendSuggestion}
              onInterrupt={() => {
                // A side command holds no shell command: cancel it without touching the terminal.
                if (sideBusy) {
                  cancelRunning();
                  return;
                }
                // At an idle prompt there is nothing to stop: no cancel, and no ^C into the user's shell.
                if (!busy) {
                  return;
                }
                cancelRunning();
                // A stop does not send what is queued: it comes back into the input.
                returnQueued();
                // ^C only when the agent holds the shell, so stopping a thinking/waiting turn never kills
                // the user's own foreground process.
                if (!locked && borderActive(agentControl)) {
                  sendRaw(CTRL_C);
                }
              }}
              restoreTick={restoreTick}
              takeReturned={takeReturned}
              onApproveFirst={approveFirst}
              onDenyFirst={denyFirst}
              onClose={closeBar}
              draftKey={conversationId ?? NEW_DRAFT_KEY}
              draftDiscard={discardDraft}
            />
          </AgentPanel>
        }
      />
      {(busy || sideBusy) && pendingApprovalItem === null && !chatBodyShown(panel.open, panelBody) && (
        <AgentActivityToast
          dock={panel.dock}
          text={activity.text}
          full={activity.full}
          onOpen={showChat}
        />
      )}
      {pendingApprovalItem !== null && !chatBodyShown(panel.open, panelBody) && (
        <ApprovalToast approval={pendingApprovalItem} dock={panel.dock} onApprove={answer} onOpen={showChat} />
      )}
      {settingsOpen && (
        <SettingsDialog
          focusModel={settingsFocusModel}
          onClose={closeSettings}
          onChanged={refreshStatus}
          onViewLoaded={refreshSettingsState}
          hideMarks={hideMarks}
          onHideMarksChange={setMarksHidden}
          queueMode={queueMode}
          onQueueModeChange={setQueueModeSetting}
          thinkingMode={thinkingMode}
          onThinkingModeChange={setThinkingModeSetting}
          suggestPrompts={suggestPrompts}
          onSuggestPromptsChange={setSuggestPromptsSetting}
          notifications={notifications}
          onNotificationsChange={setNotificationsSetting}
          devtools={devtools}
          onDevtoolsChange={setDevtoolsSetting}
        />
      )}
      {helpOpen && <HelpDialog load={helpState} onRetry={loadHelp} onClose={closeHelp} />}
      {agentPhase !== null ? (
        <IntegrateDialog key="agent" phase={agentPhase} agent={{ onDecline: declineAgent }} onStart={acceptAgent} onClose={closeAgent} local={localShell} />
      ) : (
        integrate.kind !== 'closed' && <IntegrateDialog key="user" phase={integrate} onStart={startIntegrate} onClose={closeIntegrate} local={localShell} />
      )}
      <Tooltip />
      <ContextMenu
        terminal={{ state: effective, agentBusy: busy || sideBusy, pasteRoutesToPty: keyRoute.keys === 'pty' }}
        onClearTerminal={() => tab.clear(effectiveRef.current, handleTerminalInput)}
        onFindTerminal={openTerminalFind}
        onFindChat={openChatFind}
        devtools={devtools}
      />
    </div>
    </ActiveTabContext.Provider>
  );
}

/** The state of one terminal, as it changes. Its fields are the names the UI reads (see tabState.ts). */
function useTabState(tab: TabController): TabState {
  return useSyncExternalStore(tab.subscribe, tab.getSnapshot);
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
