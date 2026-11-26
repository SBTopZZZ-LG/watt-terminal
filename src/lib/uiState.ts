// The UI state file (`ui-state.json`) on the frontend side. localStorage stays the synchronous first-paint cache. After
// startup the file is read and reconciled with it: the file wins when it has a valid value, and when it has none yet the
// local value is migrated into it. Later changes go to the file through a debounced saver. Everything here is pure, or takes
// its timers and its send function as arguments, so it is tested without Tauri.

import { layoutFromUnknown, sameLayout } from './panelLayout';
import type { PanelLayout } from './panelLayout';
import { FONT_SIZE_MAX, FONT_SIZE_MIN } from './zoom';
import type { PanelState, QueueMode, ThinkingMode, UiStatePatch } from './types';

/** What a reconcile step does: `apply` takes a remote value into the UI, `migrate` writes a local value to the file. */
export interface Reconciled<T> {
  apply: T | null;
  migrate: T | null;
}

/**
 * The panel. A valid remote panel wins; it is applied when it differs from the local layout. With no valid remote panel, the
 * local layout is migrated into the file.
 */
export function reconcilePanel(local: PanelLayout, remote: PanelState | null | undefined): Reconciled<PanelLayout> {
  const valid = layoutFromUnknown(remote);
  if (valid === null) {
    return { apply: null, migrate: local };
  }
  return { apply: sameLayout(valid, local) ? null : valid, migrate: null };
}

/** The font size. A remote value that is an integer in the font range wins; otherwise the local size is migrated. */
export function reconcileFont(local: number, remote: number | null | undefined): Reconciled<number> {
  if (typeof remote !== 'number' || !Number.isInteger(remote) || remote < FONT_SIZE_MIN || remote > FONT_SIZE_MAX) {
    return { apply: null, migrate: local };
  }
  return { apply: remote === local ? null : remote, migrate: null };
}

/** The history grouping from the file, or null when it has none (the default then applies). Nothing is migrated: it has no local copy. */
export function reconcileGroupBy(remote: string | null | undefined): 'date' | 'workspace' | null {
  return remote === 'date' || remote === 'workspace' ? remote : null;
}

/** The queue mode from the file, or null when it has none (all together then applies). Nothing is migrated: it has no local copy. */
export function reconcileQueueMode(remote: string | null | undefined): QueueMode | null {
  return remote === 'all' || remote === 'one' ? remote : null;
}

/** The thinking mode from the file, or null when it has none or an unknown one (auto then applies). Nothing is migrated. */
export function reconcileThinkingMode(remote: string | null | undefined): ThinkingMode | null {
  return remote === 'collapsed' || remote === 'auto' || remote === 'expanded' ? remote : null;
}

/** The patch that saves the thinking mode. */
export function thinkingModePatch(mode: ThinkingMode): UiStatePatch {
  return { thinking_mode: mode };
}

/** The marks setting from the file, or null when the file has no boolean for it (the default then applies). Nothing is migrated. */
export function reconcileHideMarks(remote: boolean | null | undefined): boolean | null {
  return typeof remote === 'boolean' ? remote : null;
}

/** The patch that saves the queue mode. */
export function queueModePatch(mode: QueueMode): UiStatePatch {
  return { queue_mode: mode };
}

/** The patch that saves the marks setting. */
export function hideMarksPatch(hide: boolean): UiStatePatch {
  return { hide_marks: hide };
}

/** The suggest-prompts setting from the file, or null when the file has no boolean for it (on then applies). Nothing is migrated. */
export function reconcileSuggestPrompts(remote: boolean | null | undefined): boolean | null {
  return typeof remote === 'boolean' ? remote : null;
}

/** The patch that saves the suggest-prompts setting. */
export function suggestPromptsPatch(on: boolean): UiStatePatch {
  return { suggest_prompts: on };
}

/** The notifications setting from the file, or null when the file has no boolean for it (on then applies). Nothing is migrated. */
export function reconcileNotifications(remote: boolean | null | undefined): boolean | null {
  return typeof remote === 'boolean' ? remote : null;
}

/** The patch that saves the notifications setting. */
export function notificationsPatch(on: boolean): UiStatePatch {
  return { notifications: on };
}

/** The developer (layout inspector) setting from the file, or null when the file has no boolean for it (off then applies). Nothing is migrated. */
export function reconcileDevtools(remote: boolean | null | undefined): boolean | null {
  return typeof remote === 'boolean' ? remote : null;
}

/** The patch that saves the developer (layout inspector) setting. */
export function devtoolsPatch(on: boolean): UiStatePatch {
  return { devtools: on };
}

/** The new-tabs setting from the file, or null when the file has no boolean for it (off then applies). Nothing is migrated. */
export function reconcileNewTabsVisible(remote: boolean | null | undefined): boolean | null {
  return typeof remote === 'boolean' ? remote : null;
}

/** The patch that saves the new-tabs setting (new terminals are visible to the agent). */
export function newTabsVisiblePatch(on: boolean): UiStatePatch {
  return { new_tabs_visible_to_agent: on };
}

/** The patch that saves a panel layout. */
export function panelPatch(layout: PanelLayout): UiStatePatch {
  return { panel: { dock: layout.dock, sizes: { ...layout.sizes }, open: layout.open } };
}

/** The patch that saves a font size. */
export function fontPatch(size: number): UiStatePatch {
  return { font_size: size };
}

/** How long a change waits for more changes before it is written. */
export const PATCH_DELAY_MS = 300;

/** The timer functions the saver uses. Injected, so tests can run them by hand. */
export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

const browserTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface PatchSaver {
  /** Adds a change. Changes made within the delay are merged (a later value for a field wins) and sent together. */
  queue(patch: UiStatePatch): void;
  /** Sends what is waiting now, without waiting for the timer. */
  flush(): void;
}

/**
 * Debounces UI state patches. Each queued patch restarts the timer, so a burst (a drag of the panel divider) is written
 * once, when it stops. `send` gets the merged patch and is responsible for its own errors.
 */
export function createPatchSaver(
  send: (patch: UiStatePatch) => void,
  delayMs: number = PATCH_DELAY_MS,
  timers: Timers = browserTimers,
): PatchSaver {
  let pending: UiStatePatch = {};
  let handle: unknown = null;

  const flush = () => {
    if (handle !== null) {
      timers.clear(handle);
      handle = null;
    }
    if (Object.keys(pending).length === 0) {
      return;
    }
    const patch = pending;
    pending = {};
    send(patch);
  };

  return {
    queue(patch) {
      pending = { ...pending, ...patch };
      if (handle !== null) {
        timers.clear(handle);
      }
      handle = timers.set(flush, delayMs);
    },
    flush,
  };
}

export interface RestoreInput {
  /** The file's last_conversation_id, or null. */
  lastId: string | null;
  /** The transcript's item count now. A restore only happens into an empty transcript. */
  itemCount: number;
  /** The transcript says a turn is in flight. */
  transcriptBusy: boolean;
  /** The status snapshot says the agent is busy. */
  statusBusy: boolean;
}

/** Whether to reopen the last conversation at startup: there is one, the transcript is empty, and no turn is running. */
export function shouldRestoreLast(input: RestoreInput): boolean {
  return input.lastId !== null && input.lastId !== '' && input.itemCount === 0 && !input.transcriptBusy && !input.statusBusy;
}
