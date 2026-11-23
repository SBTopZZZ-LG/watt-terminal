import { describe, expect, it } from 'vitest';
import {
  createPatchSaver,
  fontPatch,
  panelPatch,
  PATCH_DELAY_MS,
  hideMarksPatch,
  reconcileFont,
  reconcileGroupBy,
  queueModePatch,
  reconcileHideMarks,
  reconcileQueueMode,
  reconcileThinkingMode,
  thinkingModePatch,
  reconcilePanel,
  reconcileSuggestPrompts,
  suggestPromptsPatch,
  notificationsPatch,
  reconcileNotifications,
  devtoolsPatch,
  reconcileDevtools,
  shouldRestoreLast,
} from './uiState';
import type { Timers } from './uiState';
import { defaultLayout } from './panelLayout';
import type { PanelLayout } from './panelLayout';
import type { PanelState, UiStatePatch } from './types';

const LOCAL: PanelLayout = { dock: 'right', sizes: { left: 380, right: 200 }, open: true };
const REMOTE_PANEL: PanelState = { dock: 'left', sizes: { left: 300, right: null }, open: false };

describe('reconcilePanel', () => {
  it('a valid remote panel wins and is applied when it differs', () => {
    expect(reconcilePanel(LOCAL, REMOTE_PANEL)).toEqual({
      apply: { dock: 'left', sizes: { left: 300, right: null }, open: false },
      migrate: null,
    });
  });

  it('a valid remote panel equal to the local layout applies nothing', () => {
    const same: PanelState = { dock: 'right', sizes: { left: 380, right: 200 }, open: true };
    expect(reconcilePanel(LOCAL, same)).toEqual({ apply: null, migrate: null });
  });

  it('no remote panel migrates the local layout into the file', () => {
    expect(reconcilePanel(LOCAL, null)).toEqual({ apply: null, migrate: LOCAL });
    expect(reconcilePanel(LOCAL, undefined)).toEqual({ apply: null, migrate: LOCAL });
  });

  it('an invalid remote panel counts as none: the local layout is migrated over it', () => {
    const bad = { ...REMOTE_PANEL, dock: 'middle' } as unknown as PanelState;
    expect(reconcilePanel(LOCAL, bad)).toEqual({ apply: null, migrate: LOCAL });
  });
});

describe('reconcileFont', () => {
  it('a remote size in range wins and is applied when it differs', () => {
    expect(reconcileFont(13, 16)).toEqual({ apply: 16, migrate: null });
  });

  it('a remote size equal to the local one applies nothing', () => {
    expect(reconcileFont(13, 13)).toEqual({ apply: null, migrate: null });
  });

  it('no remote size migrates the local one', () => {
    expect(reconcileFont(14, null)).toEqual({ apply: null, migrate: 14 });
    expect(reconcileFont(14, undefined)).toEqual({ apply: null, migrate: 14 });
  });

  it('a remote size that is not an integer in 8..32 counts as none', () => {
    expect(reconcileFont(13, 7)).toEqual({ apply: null, migrate: 13 });
    expect(reconcileFont(13, 33)).toEqual({ apply: null, migrate: 13 });
    expect(reconcileFont(13, 12.5)).toEqual({ apply: null, migrate: 13 });
    expect(reconcileFont(13, Number.NaN)).toEqual({ apply: null, migrate: 13 });
  });
});

describe('reconcileGroupBy', () => {
  it('takes the two known values, and nothing else', () => {
    expect(reconcileGroupBy('date')).toBe('date');
    expect(reconcileGroupBy('workspace')).toBe('workspace');
    expect(reconcileGroupBy('name')).toBeNull();
    expect(reconcileGroupBy(null)).toBeNull();
    expect(reconcileGroupBy(undefined)).toBeNull();
  });
});

describe('reconcileQueueMode and queueModePatch', () => {
  it('takes the two known modes from the file, and nothing else', () => {
    expect(reconcileQueueMode('all')).toBe('all');
    expect(reconcileQueueMode('one')).toBe('one');
    expect(reconcileQueueMode('some')).toBeNull();
    expect(reconcileQueueMode(null)).toBeNull();
    expect(reconcileQueueMode(undefined)).toBeNull();
  });

  it('queueModePatch carries the mode as queue_mode', () => {
    expect(queueModePatch('one')).toEqual({ queue_mode: 'one' });
  });
});

describe('reconcileHideMarks', () => {
  it('takes a boolean from the file, either way; anything else leaves the default to apply', () => {
    expect(reconcileHideMarks(true)).toBe(true);
    expect(reconcileHideMarks(false)).toBe(false);
    expect(reconcileHideMarks(null)).toBeNull();
    expect(reconcileHideMarks(undefined)).toBeNull();
    expect(reconcileHideMarks('no' as unknown as boolean)).toBeNull();
  });
});

describe('reconcileSuggestPrompts', () => {
  it('takes a boolean from the file, either way; anything else leaves the default (off) to apply', () => {
    expect(reconcileSuggestPrompts(true)).toBe(true);
    expect(reconcileSuggestPrompts(false)).toBe(false);
    expect(reconcileSuggestPrompts(null)).toBeNull();
    expect(reconcileSuggestPrompts(undefined)).toBeNull();
    expect(reconcileSuggestPrompts('yes' as unknown as boolean)).toBeNull();
  });
});

describe('reconcileNotifications', () => {
  it('takes a boolean from the file, either way; anything else leaves the default (on) to apply', () => {
    expect(reconcileNotifications(true)).toBe(true);
    expect(reconcileNotifications(false)).toBe(false);
    expect(reconcileNotifications(null)).toBeNull();
    expect(reconcileNotifications(undefined)).toBeNull();
    expect(reconcileNotifications('no' as unknown as boolean)).toBeNull();
  });
});

describe('reconcileDevtools', () => {
  it('takes a boolean from the file, either way; anything else leaves the default (off) to apply', () => {
    expect(reconcileDevtools(true)).toBe(true);
    expect(reconcileDevtools(false)).toBe(false);
    expect(reconcileDevtools(null)).toBeNull();
    expect(reconcileDevtools(undefined)).toBeNull();
    expect(reconcileDevtools('yes' as unknown as boolean)).toBeNull();
  });
});

describe('patch builders', () => {
  it('notificationsPatch carries the boolean as notifications', () => {
    expect(notificationsPatch(true)).toEqual({ notifications: true });
    expect(notificationsPatch(false)).toEqual({ notifications: false });
  });

  it('devtoolsPatch carries the boolean as devtools', () => {
    expect(devtoolsPatch(true)).toEqual({ devtools: true });
    expect(devtoolsPatch(false)).toEqual({ devtools: false });
  });

  it('hideMarksPatch carries the boolean as hide_marks', () => {
    expect(hideMarksPatch(true)).toEqual({ hide_marks: true });
    expect(hideMarksPatch(false)).toEqual({ hide_marks: false });
  });

  it('suggestPromptsPatch carries the boolean as suggest_prompts', () => {
    expect(suggestPromptsPatch(true)).toEqual({ suggest_prompts: true });
    expect(suggestPromptsPatch(false)).toEqual({ suggest_prompts: false });
  });

  it('panelPatch carries the snake_case panel shape, with its own copy of the sizes', () => {
    const layout = defaultLayout();
    const patch = panelPatch(layout);
    expect(patch).toEqual({ panel: { dock: 'right', sizes: { left: 380, right: 380 }, open: true } });
    expect(patch.panel?.sizes).not.toBe(layout.sizes);
  });

  it('fontPatch sets font_size', () => {
    expect(fontPatch(15)).toEqual({ font_size: 15 });
  });
});

/** Timers that run only when the test says so. */
function manualTimers() {
  let nextHandle = 1;
  const scheduled = new Map<number, { fn: () => void; ms: number }>();
  const timers: Timers = {
    set(fn, ms) {
      const handle = nextHandle++;
      scheduled.set(handle, { fn, ms });
      return handle;
    },
    clear(handle) {
      scheduled.delete(handle as number);
    },
  };
  return {
    timers,
    pending: () => [...scheduled.values()].map((s) => s.ms),
    /** Runs every scheduled callback, as if their time had come. */
    fire() {
      const due = [...scheduled.values()];
      scheduled.clear();
      for (const t of due) {
        t.fn();
      }
    },
  };
}

describe('createPatchSaver', () => {
  it('sends nothing until the delay has passed, then sends once', () => {
    const sent: UiStatePatch[] = [];
    const t = manualTimers();
    const saver = createPatchSaver((p) => sent.push(p), PATCH_DELAY_MS, t.timers);
    saver.queue(fontPatch(14));
    expect(sent).toEqual([]);
    expect(t.pending()).toEqual([PATCH_DELAY_MS]);
    t.fire();
    expect(sent).toEqual([{ font_size: 14 }]);
  });

  it('a burst is merged into one patch, a later value for a field winning', () => {
    const sent: UiStatePatch[] = [];
    const t = manualTimers();
    const saver = createPatchSaver((p) => sent.push(p), PATCH_DELAY_MS, t.timers);
    saver.queue(fontPatch(14));
    saver.queue(panelPatch(defaultLayout()));
    saver.queue(fontPatch(16));
    t.fire();
    expect(sent).toHaveLength(1);
    expect(sent[0]).toEqual({ font_size: 16, panel: panelPatch(defaultLayout()).panel });
  });

  it('each change restarts the timer, so only one timer is pending', () => {
    const t = manualTimers();
    const saver = createPatchSaver(() => undefined, PATCH_DELAY_MS, t.timers);
    saver.queue(fontPatch(14));
    saver.queue(fontPatch(15));
    saver.queue(fontPatch(16));
    expect(t.pending()).toHaveLength(1);
  });

  it('flush sends now and cancels the timer', () => {
    const sent: UiStatePatch[] = [];
    const t = manualTimers();
    const saver = createPatchSaver((p) => sent.push(p), PATCH_DELAY_MS, t.timers);
    saver.queue(fontPatch(9));
    saver.flush();
    expect(sent).toEqual([{ font_size: 9 }]);
    expect(t.pending()).toEqual([]);
    t.fire();
    expect(sent).toHaveLength(1);
  });

  it('flush with nothing queued sends nothing', () => {
    const sent: UiStatePatch[] = [];
    const saver = createPatchSaver((p) => sent.push(p), PATCH_DELAY_MS, manualTimers().timers);
    saver.flush();
    expect(sent).toEqual([]);
  });

  it('a later change after a send starts a new batch', () => {
    const sent: UiStatePatch[] = [];
    const t = manualTimers();
    const saver = createPatchSaver((p) => sent.push(p), PATCH_DELAY_MS, t.timers);
    saver.queue(fontPatch(10));
    t.fire();
    saver.queue(fontPatch(11));
    t.fire();
    expect(sent).toEqual([{ font_size: 10 }, { font_size: 11 }]);
  });

  it('uses the default delay of 300 ms', () => {
    const t = manualTimers();
    createPatchSaver(() => undefined, undefined, t.timers).queue(fontPatch(12));
    expect(t.pending()).toEqual([300]);
    expect(PATCH_DELAY_MS).toBe(300);
  });
});

describe('shouldRestoreLast', () => {
  const ready = { lastId: 'ses_1', itemCount: 0, transcriptBusy: false, statusBusy: false };

  it('restores when there is a last conversation, the transcript is empty, and no turn is running', () => {
    expect(shouldRestoreLast(ready)).toBe(true);
  });

  it('does not restore without a last conversation, or with an empty id', () => {
    expect(shouldRestoreLast({ ...ready, lastId: null })).toBe(false);
    expect(shouldRestoreLast({ ...ready, lastId: '' })).toBe(false);
  });

  it('does not restore into a transcript that has items', () => {
    expect(shouldRestoreLast({ ...ready, itemCount: 1 })).toBe(false);
  });

  it('does not restore while a turn is in flight, on either side', () => {
    expect(shouldRestoreLast({ ...ready, transcriptBusy: true })).toBe(false);
    expect(shouldRestoreLast({ ...ready, statusBusy: true })).toBe(false);
  });
});

describe('reconcileThinkingMode and thinkingModePatch', () => {
  it('reads the three modes, and null for anything else (the default then applies)', () => {
    expect(reconcileThinkingMode('collapsed')).toBe('collapsed');
    expect(reconcileThinkingMode('auto')).toBe('auto');
    expect(reconcileThinkingMode('expanded')).toBe('expanded');
    expect(reconcileThinkingMode('open')).toBeNull();
    expect(reconcileThinkingMode(null)).toBeNull();
    expect(reconcileThinkingMode(undefined)).toBeNull();
  });

  it('thinkingModePatch carries the mode as thinking_mode', () => {
    expect(thinkingModePatch('expanded')).toEqual({ thinking_mode: 'expanded' });
  });
});
