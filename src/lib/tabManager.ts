// The registry of terminals (tabs) and the one connection to the chat. A terminal's own events (its channel) go to that
// terminal's controller, and to the tab listeners after it. Agent events come on the chat channel and go to the chat
// listeners only. Errors from any part go to the chat listeners too, where the transcript shows them.

import { Channel } from '@tauri-apps/api/core';
import { api, CHAT_ID, errorText, SESSION_ID } from './api';
import { TabController } from './tab';
import type { TabDeps } from './tab';
import { allocTabId, neighbourAfterClose, tabLimitReached } from './tabStrip';
import { pickTabTitle } from './tabState';
import { clampFontSize, defaultSizeStorage, FONT_SIZE_DEFAULT, saveFontSize, stepFontSize } from './zoom';
import type { SessionEvent, Status, TabInfo, WireEvent } from './types';

export type Incoming = { kind: 'event'; event: WireEvent } | { kind: 'error'; message: string };
/** Called with each chat message and the id of the chat it is for (errors are not chat-scoped: see reportError). */
export type ChatListener = (msg: Incoming, chatId: string) => void;
export type TabEventListener = (tab: TabController, ev: SessionEvent) => void;

/** What the strip and the App render from the registry. Replaced on every change, so the same object means no change. */
export interface TabsSnapshot {
  /** The tab ids, in order. */
  readonly ids: readonly string[];
  readonly activeId: string | null;
}

export class TabManager {
  private tabs = new Map<string, TabController>();
  /** The number each tab was shown with when it was created ("Terminal N"). Never renumbered. */
  private ordinals = new Map<string, number>();
  private activeId: string | null = null;
  /** The number the next created tab id starts from. `main` is the first tab, so the first created one is `t2`. */
  private counter = 2;
  private snapshot: TabsSnapshot = { ids: [], activeId: null };
  /** The startup, run once (see start). */
  private started: Promise<Status | null> | null = null;
  private readonly chatListeners = new Set<ChatListener>();
  private readonly tabListeners = new Set<TabEventListener>();
  private readonly storeListeners = new Set<() => void>();
  private readonly fontListeners = new Set<(size: number) => void>();
  /** Each chat's agent channel, by chat id: the attach in flight or done. A failed attach is removed (see attachChat). */
  private readonly chatAttach = new Map<string, Promise<void>>();

  /** The terminal with this id, created on first use. Later calls return the same controller. */
  ensure(id: string): TabController {
    const existing = this.tabs.get(id);
    if (existing !== undefined) {
      return existing;
    }
    const tab = this.build(id);
    this.tabs.set(id, tab);
    this.ordinals.set(id, this.tabs.size);
    if (this.activeId === null) {
      this.activeId = id;
    }
    this.notifyStore();
    return tab;
  }

  get(id: string): TabController | undefined {
    return this.tabs.get(id);
  }

  /** Every terminal, in tab-strip order. */
  list(): TabController[] {
    return [...this.tabs.values()];
  }

  /** The number a tab was created with ("Terminal N" when it has no other title). 1 for an unknown id. */
  ordinal(id: string): number {
    return this.ordinals.get(id) ?? 1;
  }

  /** The title a terminal shows in the strip and in agent events (see pickTabTitle). Null for an unknown id. */
  label(id: string): string | null {
    const tab = this.tabs.get(id);
    if (tab === undefined) {
      return null;
    }
    const s = tab.getSnapshot();
    return pickTabTitle({ xtermTitle: s.title, foreground: s.input.foreground, cwd: s.cwd, index: this.ordinal(id) });
  }

  /** The tab the UI shows. Null before any tab exists. */
  get active(): TabController | null {
    return this.activeId === null ? null : (this.tabs.get(this.activeId) ?? null);
  }

  setActive(id: string): void {
    if (!this.tabs.has(id)) {
      throw new Error(`no tab ${id}`);
    }
    if (this.activeId === id) {
      return;
    }
    this.activeId = id;
    this.notifyStore();
  }

  /** Called when the tab list or the active tab changes. For useSyncExternalStore. */
  readonly subscribeStore = (listener: () => void): (() => void) => {
    this.storeListeners.add(listener);
    return () => {
      this.storeListeners.delete(listener);
    };
  };

  /** The registry's snapshot, for useSyncExternalStore (see TabsSnapshot). */
  readonly getSnapshot = (): TabsSnapshot => this.snapshot;

  /**
   * Startup. Reads the terminals the backend still has (they outlive a page reload), makes a controller for each in the
   * backend's order, connects each and reads its status. With none, it starts `main`. Resolves with the active terminal's
   * status, or null when that one did not connect (the error is shown). Runs once; later calls get the same promise.
   */
  start(): Promise<Status | null> {
    if (this.started === null) {
      this.started = this.runStart();
    }
    return this.started;
  }

  /**
   * A new terminal, with its shell started, made active. Null at the tab limit, or when its shell did not start (the error
   * is shown). The new terminal belongs to no chat until it is assigned.
   */
  async create(): Promise<TabController | null> {
    if (tabLimitReached(this.tabs.size)) {
      return null;
    }
    const { id, counter } = allocTabId(this.counter, new Set(this.tabs.keys()));
    this.counter = counter;
    const previous = this.activeId;
    const tab = this.ensure(id);
    this.setActive(id);
    try {
      await this.connectTab(tab);
    } catch (e) {
      this.drop(id);
      if (previous !== null && this.tabs.has(previous)) {
        this.setActive(previous);
      }
      // The shell may have started before the attach failed; closing it is best effort.
      api.closeSession(id).catch(() => undefined);
      this.reportError(e);
      return null;
    }
    void this.refresh().catch((e: unknown) => this.reportError(e));
    return tab;
  }

  /**
   * A new terminal, made visible to chat `chatId` when `visibleToAgent` (the new-tabs setting). Null at the tab limit, or when
   * its shell did not start (the error is shown). The assignment's refusal is shown too; the terminal then stays private.
   */
  async createTab(visibleToAgent: boolean, chatId: string): Promise<TabController | null> {
    const tab = await this.create();
    if (tab !== null && visibleToAgent) {
      await this.assign(tab.id, true, chatId);
    }
    return tab;
  }

  /**
   * Assigns a terminal to chat `chatId` (`on`), or unassigns it. The backend refuses while the chat is busy and would lose the
   * terminal; the refusal is shown. Then the owners are read again.
   */
  async assign(id: string, on: boolean, chatId: string): Promise<void> {
    try {
      await api.assignTab(chatId, id, on);
    } catch (e) {
      this.reportError(e);
    }
    await this.refresh().catch((e: unknown) => this.reportError(e));
  }

  /**
   * Moves a terminal that chat `from` owns to chat `chatId`: it is unassigned there, then assigned here. A refusal (`from` is
   * busy and would lose it) is shown, and nothing more is done. The owners are read again either way, so the strip shows what
   * happened.
   */
  async move(id: string, from: string, chatId: string): Promise<void> {
    try {
      await api.assignTab(from, id, false);
      await api.assignTab(chatId, id, true);
    } catch (e) {
      this.reportError(e);
    }
    await this.refresh().catch((e: unknown) => this.reportError(e));
  }

  /**
   * Closes a terminal: its session is closed, then its controller is disposed. The last terminal cannot be closed. When the
   * active one closes, its neighbour takes over. Resolves true when it closed.
   */
  async close(id: string): Promise<boolean> {
    if (this.tabs.size <= 1 || !this.tabs.has(id)) {
      return false;
    }
    try {
      await api.closeSession(id);
    } catch (e) {
      this.reportError(e);
      return false;
    }
    this.drop(id);
    void this.refresh().catch((e: unknown) => this.reportError(e));
    return true;
  }

  /** Reads list_tabs again and sets each known terminal's owner. Call after a create, a close or an assignment. */
  async refresh(): Promise<void> {
    this.applyOwners(await api.listTabs());
  }

  /**
   * A new chat (no terminal, a new conversation), with its agent channel attached at once so none of its events are lost.
   * Resolves with its id. Not used by the UI yet (multi-chat D2).
   */
  async createChat(): Promise<string> {
    const chatId = await api.createChat();
    await this.ensureChatAttached(chatId);
    return chatId;
  }

  /** Attaches a chat's agent channel. Once per chat: a later call for the same chat reuses the attach (see attachChat). */
  ensureChatAttached(chatId: string): Promise<void> {
    return this.attachChat(chatId, false);
  }

  /** Agent events and errors. Events that arrive while no listener is subscribed are not replayed. */
  readonly subscribeChat = (listener: ChatListener): (() => void) => {
    this.chatListeners.add(listener);
    return () => {
      this.chatListeners.delete(listener);
    };
  };

  /** Terminal events, after the owning controller has reduced them. */
  readonly subscribeTabEvents = (listener: TabEventListener): (() => void) => {
    this.tabListeners.add(listener);
    return () => {
      this.tabListeners.delete(listener);
    };
  };

  /**
   * Shows an error to the user (through the chat listeners). Errors are not chat-scoped (a terminal's failure is not a chat's),
   * so they carry the default chat's id; the App shows them in whichever chat is active.
   */
  reportError(e: unknown): void {
    this.emitChat({ kind: 'error', message: errorText(e) }, CHAT_ID);
  }

  /** The font size xterm is drawing with (all tabs share it). */
  currentFontSize(): number {
    return this.list()[0]?.fontSize ?? FONT_SIZE_DEFAULT;
  }

  /** Calls `listener` with the new size after each change made in the UI (zoom keys). Returns the unsubscribe. */
  onFontSizeChange(listener: (size: number) => void): () => void {
    this.fontListeners.add(listener);
    return () => {
      this.fontListeners.delete(listener);
    };
  }

  /**
   * Sets the font size of every tab and saves it. `notify` is false when the size came from the UI state file: there is
   * nothing to write back.
   */
  setFontSize(size: number, notify: boolean): void {
    const next = clampFontSize(size);
    if (next === this.currentFontSize()) {
      return;
    }
    for (const tab of this.list()) {
      tab.applyFontSize(next);
    }
    saveFontSize(next, defaultSizeStorage());
    if (notify) {
      for (const listener of [...this.fontListeners]) {
        listener(next);
      }
    }
  }

  /** Changes the font size by one step, or back to the default, and saves it. Steps past the clamp do nothing. */
  zoomFont(action: 'in' | 'out' | 'reset'): void {
    const current = this.currentFontSize();
    const next = action === 'reset' ? FONT_SIZE_DEFAULT : stepFontSize(current, action === 'in' ? 1 : -1);
    if (next === current) {
      return;
    }
    this.setFontSize(next, true);
  }

  private emitChat(msg: Incoming, chatId: string): void {
    for (const listener of [...this.chatListeners]) {
      listener(msg, chatId);
    }
  }

  private notifyStore(): void {
    this.snapshot = { ids: [...this.tabs.keys()], activeId: this.activeId };
    for (const listener of [...this.storeListeners]) {
      listener();
    }
  }

  /** A controller for `id`, wired to this registry. Not added to the map (see ensure and adopt). */
  private build(id: string): TabController {
    const deps: TabDeps = {
      onError: (e) => this.reportError(e),
      onTabEvent: (tab, ev) => {
        for (const listener of [...this.tabListeners]) {
          listener(tab, ev);
        }
      },
      // The backend sends no agent event on a terminal's channel; should one come, it is the default chat's (as before).
      onStrayEvent: (event) => this.emitChat({ kind: 'event', event }, CHAT_ID),
      attachChat: (fresh) => this.attachKnownChats(fresh),
    };
    return new TabController(id, deps);
  }

  private async runStart(): Promise<Status | null> {
    let infos: TabInfo[] = [];
    try {
      infos = await api.listTabs();
    } catch (e) {
      // Without the list, start `main` as a fresh page does. The error is shown.
      this.reportError(e);
    }
    this.adopt(infos.length > 0 ? infos.map((t) => t.id) : [SESSION_ID]);
    this.applyOwners(infos);
    const tabs = this.list();
    const statuses = await Promise.all(
      tabs.map((tab) =>
        this.connectTab(tab).catch((e: unknown) => {
          this.reportError(e);
          return null;
        }),
      ),
    );
    const at = tabs.findIndex((tab) => tab.id === this.activeId);
    return at === -1 ? null : (statuses[at] ?? null);
  }

  /** Starts or re-attaches a terminal's shell, then reads its status into the terminal. */
  private async connectTab(tab: TabController): Promise<Status> {
    await tab.connect();
    // The chat fields are the default chat's for now. Multi-chat D2 reads the active chat's (see App).
    const status = await api.getStatus(CHAT_ID, tab.id);
    tab.applyStatus(status);
    return status;
  }

  /**
   * Makes the registry hold exactly `ids`, in that order. Controllers already here keep their state. Any other is dropped:
   * at startup nothing but the placeholder `main` exists, and it has not connected.
   */
  private adopt(ids: readonly string[]): void {
    const next = new Map<string, TabController>();
    for (const id of ids) {
      next.set(id, this.tabs.get(id) ?? this.build(id));
    }
    for (const [id, tab] of this.tabs) {
      if (!next.has(id)) {
        tab.dispose();
      }
    }
    const ordinals = new Map<string, number>();
    ids.forEach((id, i) => ordinals.set(id, this.ordinals.get(id) ?? i + 1));
    this.tabs = next;
    this.ordinals = ordinals;
    if (this.activeId === null || !next.has(this.activeId)) {
      this.activeId = ids[0] ?? null;
    }
    this.notifyStore();
  }

  /** Removes a terminal and disposes its controller. When it was the active one, its neighbour takes over. */
  private drop(id: string): void {
    const tab = this.tabs.get(id);
    if (tab === undefined) {
      return;
    }
    if (this.activeId === id) {
      this.activeId = neighbourAfterClose([...this.tabs.keys()], id);
    }
    this.tabs.delete(id);
    this.ordinals.delete(id);
    this.notifyStore();
    tab.dispose();
  }

  private applyOwners(infos: readonly TabInfo[]): void {
    for (const info of infos) {
      this.tabs.get(info.id)?.setOwner(info.owner);
    }
  }

  /**
   * Attaches one chat's agent channel. Memoised per chat while the attach has succeeded; `fresh` attaches again (the channel
   * is replaced). A failed attach is removed, so the next call retries it. The channel's closure tags every event with the
   * chat's id, which is how the listeners know which chat an event is for.
   */
  private attachChat(chatId: string, fresh: boolean): Promise<void> {
    const existing = this.chatAttach.get(chatId);
    if (!fresh && existing !== undefined) {
      return existing;
    }
    const channel = new Channel<WireEvent>((event) => this.emitChat({ kind: 'event', event }, chatId));
    const attach: Promise<void> = api.attachChat(chatId, channel).catch((e: unknown) => {
      if (this.chatAttach.get(chatId) === attach) {
        this.chatAttach.delete(chatId);
      }
      throw e;
    });
    this.chatAttach.set(chatId, attach);
    return attach;
  }

  /**
   * The attach a terminal's restart asks for (TabDeps.attachChat): every chat this registry knows, the default chat always,
   * each attached as `fresh` says. The backend's attach is idempotent, so a fresh one only replaces the channel.
   */
  private attachKnownChats(fresh: boolean): Promise<void> {
    const chats = new Set([CHAT_ID, ...this.chatAttach.keys()]);
    return Promise.all([...chats].map((chatId) => this.attachChat(chatId, fresh))).then(() => undefined);
  }
}

/** The one registry the app uses. */
export const tabs = new TabManager();

/** Shows an error to the user. Shorthand for tabs.reportError. */
export function reportError(e: unknown): void {
  tabs.reportError(e);
}
