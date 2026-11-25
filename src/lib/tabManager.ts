// The registry of terminals (tabs) and the one connection to the chat. A terminal's own events (its channel) go to that
// terminal's controller, and to the tab listeners after it. Agent events come on the chat channel and go to the chat
// listeners only. Errors from any part go to the chat listeners too, where the transcript shows them.

import { Channel } from '@tauri-apps/api/core';
import { api, CHAT_ID, errorText } from './api';
import { TabController } from './tab';
import type { TabDeps } from './tab';
import { clampFontSize, defaultSizeStorage, FONT_SIZE_DEFAULT, saveFontSize, stepFontSize } from './zoom';
import type { SessionEvent, WireEvent } from './types';

export type Incoming = { kind: 'event'; event: WireEvent } | { kind: 'error'; message: string };
export type ChatListener = (msg: Incoming) => void;
export type TabEventListener = (tab: TabController, ev: SessionEvent) => void;

export class TabManager {
  private readonly tabs = new Map<string, TabController>();
  private activeId: string | null = null;
  private readonly chatListeners = new Set<ChatListener>();
  private readonly tabListeners = new Set<TabEventListener>();
  private readonly storeListeners = new Set<() => void>();
  private readonly fontListeners = new Set<(size: number) => void>();
  /** The chat channel, attached once. Null until the first attach is started. */
  private chatAttach: Promise<void> | null = null;

  /** The terminal with this id, created on first use. Later calls return the same controller. */
  ensure(id: string): TabController {
    const existing = this.tabs.get(id);
    if (existing !== undefined) {
      return existing;
    }
    const deps: TabDeps = {
      onError: (e) => this.reportError(e),
      onTabEvent: (tab, ev) => {
        for (const listener of [...this.tabListeners]) {
          listener(tab, ev);
        }
      },
      onStrayEvent: (event) => this.emitChat({ kind: 'event', event }),
      attachChat: () => this.attachChat(),
    };
    const tab = new TabController(id, deps);
    this.tabs.set(id, tab);
    if (this.activeId === null) {
      this.activeId = id;
    }
    this.notifyStore();
    return tab;
  }

  get(id: string): TabController | undefined {
    return this.tabs.get(id);
  }

  list(): TabController[] {
    return [...this.tabs.values()];
  }

  /** The tab the UI shows. Null before any tab exists. */
  get active(): TabController | null {
    return this.activeId === null ? null : (this.tabs.get(this.activeId) ?? null);
  }

  setActive(id: string): void {
    if (!this.tabs.has(id)) {
      throw new Error(`no tab ${id}`);
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

  /** Shows an error to the user (through the chat listeners). */
  reportError(e: unknown): void {
    this.emitChat({ kind: 'error', message: errorText(e) });
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

  private emitChat(msg: Incoming): void {
    for (const listener of [...this.chatListeners]) {
      listener(msg);
    }
  }

  private notifyStore(): void {
    for (const listener of [...this.storeListeners]) {
      listener();
    }
  }

  /** Attaches the chat's agent channel once. A failed attach is retried by the next call. */
  private attachChat(): Promise<void> {
    if (this.chatAttach === null) {
      const channel = new Channel<WireEvent>((event) => this.emitChat({ kind: 'event', event }));
      this.chatAttach = api.attachChat(CHAT_ID, channel).catch((e: unknown) => {
        this.chatAttach = null;
        throw e;
      });
    }
    return this.chatAttach;
  }
}

/** The one registry the app uses. */
export const tabs = new TabManager();

/** Shows an error to the user. Shorthand for tabs.reportError. */
export function reportError(e: unknown): void {
  tabs.reportError(e);
}
