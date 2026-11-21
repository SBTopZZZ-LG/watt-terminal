import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { InputBar } from './InputBar';
import type { BarMode } from '../lib/inputState';
import type { QueuedMessage } from '../lib/messageQueue';
import type { QueueMode } from '../lib/types';

const NOOP = () => undefined;

const BASE = {
  mode: 'chat' as BarMode,
  hint: null as string | null,
  hintInline: false,
  hasPendingApproval: false,
  canApprove: false,
  busy: false,
  locked: false,
  autoApprove: false,
  onToggleAutoApprove: NOOP,
  focusToken: 0,
  onFocusChange: NOOP,
  onRestart: NOOP,
  restarting: false,
  onAgent: async () => null,
  modelVision: null as boolean | null,
  queue: [] as QueuedMessage[],
  queueMode: 'all' as QueueMode,
  queueable: false,
  onUnqueue: () => null,
  suggestions: [] as string[],
  onSuggestion: NOOP,
  onInterrupt: NOOP,
  onApproveFirst: NOOP,
  onDenyFirst: NOOP,
  onClose: NOOP,
  loadCommands: async () => ({ commands: [], project_dir: null, global_dir: null }),
};

type Props = Partial<typeof BASE> & { autoSuspended?: boolean };

const render = (over: Props = {}) => renderToStaticMarkup(<InputBar {...BASE} {...over} />);

/** The opening tag of the element that holds `needle`: searches back from the needle to the nearest `<tag`. */
function tagHolding(html: string, tag: string, needle: string): string {
  const at = html.indexOf(needle);
  expect(at, `${needle} is in the markup`).toBeGreaterThan(-1);
  const start = html.lastIndexOf(`<${tag}`, at);
  const end = html.indexOf('>', start);
  return html.slice(start, end + 1);
}

describe('InputBar: the agent label', () => {
  it('has no "agent" label beside the input', () => {
    const html = render();
    expect(html).not.toContain('mode-agent');
    expect(html).not.toContain('bar-mode');
    expect(html).not.toMatch(/>agent</);
  });
});

describe('InputBar: the field sizes to its text', () => {
  it('has a hidden mirror that holds the placeholder while the input is empty', () => {
    const html = render();
    const tag = tagHolding(html, 'div', 'bar-mirror');
    expect(tag).toContain('aria-hidden="true"');
    expect(html).toContain('bar-mirror" aria-hidden="true">Ask the agent… </div>');
    // The shortcut hint is the input's tooltip, not its placeholder.
    expect(tagHolding(html, 'textarea', 'bar-input')).toContain('title="Ctrl/Cmd+Shift+A to focus, Esc to return"');
  });

  it('the mirror holds the inline hint when the hint is the placeholder', () => {
    const html = render({ hint: 'Starting a command…', hintInline: true });
    expect(html).toContain('bar-mirror" aria-hidden="true">Starting a command… </div>');
    expect(html).not.toContain('Ask the agent');
  });

  it('the mirror is not used for the exited or collapsed modes', () => {
    expect(render({ mode: 'exited' })).not.toContain('bar-mirror');
    expect(render({ mode: 'collapsed' })).not.toContain('bar-mirror');
  });
});

describe('InputBar: the input', () => {
  it('is a textarea with one row, and keeps the bar-input class', () => {
    const html = render();
    const tag = tagHolding(html, 'textarea', 'bar-input');
    expect(tag).toContain('rows="1"');
    // The only <input> is the hidden file input of the attach button, which is not a text field.
    expect(html).not.toMatch(/<input(?! type="file")/);
  });

  it('is read-only, not disabled, while locked, so it keeps focus', () => {
    const locked = tagHolding(render({ locked: true }), 'textarea', 'bar-input');
    expect(locked).toContain('readOnly=""');
    expect(locked).not.toContain('disabled=""');
    expect(locked).toContain('is-locked');
    const free = tagHolding(render(), 'textarea', 'bar-input');
    expect(free).not.toContain('readOnly=""');
    expect(free).not.toContain('disabled=""');
    expect(free).not.toContain('is-locked');
  });
});

describe('InputBar: the send button', () => {
  it('is Send, and disabled when the input is empty', () => {
    const tag = tagHolding(render(), 'button', 'bar-send');
    expect(tag).toContain('aria-label="Send"');
    expect(tag).toContain('disabled=""');
  });

  it('is disabled while locked, even when busy', () => {
    expect(tagHolding(render({ locked: true }), 'button', 'bar-send')).toContain('disabled=""');
    expect(tagHolding(render({ locked: true, busy: true }), 'button', 'bar-send')).toContain('disabled=""');
  });

  it('is Stop, enabled, and styled as a stop while the agent is busy', () => {
    const tag = tagHolding(render({ busy: true }), 'button', 'bar-send');
    expect(tag).toContain('aria-label="Stop"');
    expect(tag).toContain('is-stop');
    expect(tag).not.toContain('disabled=""');
  });

  it('is not Stop when idle', () => {
    const tag = tagHolding(render(), 'button', 'bar-send');
    expect(tag).not.toContain('is-stop');
  });

  it('is a plain button, so it does not submit a form', () => {
    expect(tagHolding(render(), 'button', 'bar-send')).toContain('type="button"');
  });
});

describe('InputBar: the busy light', () => {
  it('the field has the animated class only while busy', () => {
    expect(render({ busy: true })).toContain('class="bar-field is-busy"');
    expect(render({ busy: false })).not.toContain('is-busy');
  });
});

describe('InputBar: the shell modes have no input', () => {
  it('the exited mode has no textarea and no send button', () => {
    const html = render({ mode: 'exited' });
    expect(html).not.toContain('<textarea');
    expect(html).not.toContain('bar-send');
  });

  it('the collapsed mode has no textarea and no send button', () => {
    const html = render({ mode: 'collapsed' });
    expect(html).not.toContain('<textarea');
    expect(html).not.toContain('bar-send');
  });
});

describe('InputBar: attaching files', () => {
  it('has a paperclip button, left of the field, that is a plain button', () => {
    const html = render();
    const tag = tagHolding(html, 'button', 'bar-attach-button');
    expect(tag).toContain('aria-label="Attach files"');
    expect(tag).toContain('type="button"');
    expect(tag).not.toContain('disabled=""');
    expect(html.indexOf('bar-attach-button')).toBeLessThan(html.indexOf('bar-field'));
  });

  it('has a hidden file input that takes several files', () => {
    const tag = tagHolding(render(), 'input', 'type="file"');
    expect(tag).toContain('multiple=""');
    expect(tag).toContain('hidden=""');
  });

  it('the paperclip is disabled while locked', () => {
    expect(tagHolding(render({ locked: true }), 'button', 'bar-attach-button')).toContain('disabled=""');
  });

  it('shows no chips row before any file is attached', () => {
    expect(render()).not.toContain('bar-attachments');
  });

  it('the exited and collapsed modes have no attach button', () => {
    expect(render({ mode: 'exited' })).not.toContain('bar-attach-button');
    expect(render({ mode: 'collapsed' })).not.toContain('bar-attach-button');
  });

  it('is not highlighted as a drop target at rest', () => {
    expect(render()).not.toContain('is-dragging');
  });
});

describe('InputBar: the approval row', () => {
  it('shows a muted approval label with a manual toggle in chat mode', () => {
    const html = render();
    expect(html).toContain('class="bar-approval"');
    expect(html).toContain('>approval:</span>');
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(tag).toContain('type="button"');
    expect(tag).toContain('aria-pressed="false"');
    expect(tag).toContain('data-tip="manual: each command asks for approval. Click (or Shift+Tab) to bypass approval for this session."');
    expect(tag).not.toContain('title=');
    expect(tag).not.toContain('is-bypass');
    expect(tag).not.toContain('is-suspended');
    expect(tag).not.toContain('disabled=""');
    expect(html).toContain('>manual</button>');
    // The toggle reads as small text: no border, not even a dashed one.
    expect(tag).not.toContain('border');
  });

  it('reads bypass, pressed and orange, with the warning in its tip, when bypass is on here', () => {
    const html = render({ autoApprove: true });
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(tag).toContain('aria-pressed="true"');
    expect(tag).toContain('is-bypass');
    expect(tag).not.toContain('is-suspended');
    expect(tag).toContain(
      'data-tip="Bypass is ON: the agent runs every command without asking, even ones your deny rules or the built-in checks would block. It resets when Watt restarts. Click to turn off."',
    );
    expect(html).toContain('>bypass</button>');
  });

  it('reads manual, not orange, with its own tip when bypass was switched on in another shell', () => {
    const html = render({ autoApprove: true, autoSuspended: true });
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(html).toContain('>manual</button>');
    expect(tag).toContain('aria-pressed="false"');
    expect(tag).toContain('is-suspended');
    expect(tag).not.toContain('is-bypass');
    expect(tag).toContain(
      'data-tip="Bypass was turned on in another shell, so it is off in this one: every command here asks for your approval. Click to turn it on here."',
    );
  });

  it('puts the same tip on the row too, so a disabled chip still shows it, and has no native title', () => {
    const html = render({ autoApprove: true, locked: true });
    const row = tagHolding(html, 'div', 'class="bar-approval"');
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(row).toContain('data-tip="Bypass is ON:');
    expect(tag).toContain('data-tip="Bypass is ON:');
    expect(tag).toContain('disabled=""');
    expect(row).not.toContain('title=');
    expect(tag).not.toContain('title=');
  });

  it('is disabled while locked', () => {
    expect(tagHolding(render({ locked: true }), 'button', 'bar-approval-toggle')).toContain('disabled=""');
  });

  it('sits above the input row', () => {
    const html = render();
    expect(html.indexOf('bar-approval')).toBeLessThan(html.indexOf('bar-field'));
  });

  it('is not shown in the exited or collapsed modes while bypass is off', () => {
    expect(render({ mode: 'exited' })).not.toContain('bar-approval');
    expect(render({ mode: 'collapsed' })).not.toContain('bar-approval');
  });

  it('collapsed: shows the bypass chip, orange, with the bypass tip, when bypass is on here', () => {
    const html = render({ mode: 'collapsed', autoApprove: true });
    expect(html).toContain('class="bar-approval is-inline"');
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(tag).toContain('is-bypass');
    expect(tag).toContain('aria-pressed="true"');
    expect(tag).toContain('data-tip="Bypass is ON:');
    expect(html).toContain('>bypass</button>');
    expect(html).not.toContain('bar-field');
    expect(html).not.toContain('<textarea');
  });

  it('collapsed: no chip when bypass is off or only suspended in another shell', () => {
    expect(render({ mode: 'collapsed', autoApprove: false })).not.toContain('bar-approval');
    expect(render({ mode: 'collapsed', autoApprove: true, autoSuspended: true })).not.toContain('bar-approval');
  });

  it('exited: shows the bypass chip, orange, when bypass is on here, beside the restart button', () => {
    const html = render({ mode: 'exited', autoApprove: true });
    const tag = tagHolding(html, 'button', 'bar-approval-toggle');
    expect(tag).toContain('is-bypass');
    expect(tag).toContain('aria-pressed="true"');
    expect(html).toContain('>bypass</button>');
    expect(html.indexOf('bar-approval')).toBeLessThan(html.indexOf('bar-restart'));
    expect(html).toContain('Start new session');
  });

  it('exited: no chip when bypass is off or only suspended', () => {
    expect(render({ mode: 'exited', autoApprove: false })).not.toContain('bar-approval');
    expect(render({ mode: 'exited', autoApprove: true, autoSuspended: true })).not.toContain('bar-approval');
  });

  it('the collapsed and exited chips are disabled while locked', () => {
    expect(tagHolding(render({ mode: 'exited', autoApprove: true, locked: true }), 'button', 'bar-approval-toggle')).toContain('disabled=""');
    expect(tagHolding(render({ mode: 'collapsed', autoApprove: true, locked: true }), 'button', 'bar-approval-toggle')).toContain('disabled=""');
  });
});

describe('InputBar: the vision note', () => {
  it('is not shown with an empty draft, whatever the model says (the note needs an image in the draft)', () => {
    expect(render({ modelVision: false })).not.toContain('bar-vision');
    expect(render({ modelVision: true })).not.toContain('bar-vision');
    expect(render()).not.toContain('bar-vision');
  });
});

describe('InputBar: the queue', () => {
  const QUEUED: QueuedMessage[] = [
    { id: 1, text: 'check the tests', attachments: [] },
    { id: 2, text: '', attachments: [{ name: 'a.txt', mime: 'text/plain', kind: 'text', data_b64: 'aGk=' }] },
  ];

  it('the placeholder says the message is queued while the agent works, and asks otherwise', () => {
    expect(render({ busy: true, queueable: true })).toContain('bar-mirror" aria-hidden="true">Queue message for the agent… </div>');
    expect(tagHolding(render({ busy: true, queueable: true }), 'textarea', 'bar-input')).toContain('placeholder="Queue message for the agent…"');
    expect(render({ busy: false })).toContain('placeholder="Ask the agent…"');
  });

  it('a side command or a lock keeps the placeholder as asking, even while busy', () => {
    expect(render({ busy: true, queueable: false })).toContain('placeholder="Ask the agent…"');
    expect(render({ busy: true, queueable: true, locked: true })).toContain('placeholder="Ask the agent…"');
  });

  it('renders no queue when empty', () => {
    expect(render()).not.toContain('bar-queue');
  });

  it('lists each queued message with its header for the mode, and a remove button', () => {
    const html = render({ queue: QUEUED, queueMode: 'all' });
    expect(html).toContain('class="bar-queue-head">Queued · sent together when the agent finishes</span>');
    expect(html).toContain('bar-queue-text" data-tip="check the tests" data-tip-clip="">check the tests</span>');
    expect(html).toContain('bar-queue-text" data-tip="Attachments only" data-tip-clip="">Attachments only</span>');
    expect(html).toContain('1 attachment</span>');
    expect(html.match(/aria-label="Remove queued message"/g)).toHaveLength(2);
    expect(tagHolding(html, 'button', 'bar-queue-remove')).not.toContain('disabled=""');
  });

  it('the header names the one-at-a-time mode', () => {
    expect(render({ queue: QUEUED, queueMode: 'one' })).toContain('Queued · sent one at a time when the agent finishes');
  });

  it('the remove buttons are disabled while the bar is locked', () => {
    const html = render({ queue: QUEUED, queueMode: 'all', locked: true });
    expect(tagHolding(html, 'button', 'bar-queue-remove')).toContain('disabled=""');
  });

  it('sits above the input row, below the approval row', () => {
    const html = render({ queue: QUEUED, queueMode: 'all' });
    expect(html.indexOf('bar-approval')).toBeLessThan(html.indexOf('bar-queue'));
    expect(html.indexOf('bar-queue')).toBeLessThan(html.indexOf('bar-field'));
  });

  it('is not shown in the exited or collapsed modes', () => {
    expect(render({ mode: 'exited', queue: QUEUED, queueMode: 'all' })).not.toContain('bar-queue');
    expect(render({ mode: 'collapsed', queue: QUEUED, queueMode: 'all' })).not.toContain('bar-queue');
  });
});

describe('InputBar: suggested prompts', () => {
  const SUGGESTED = ['Run the tests', 'Show the diff', 'Ship it'];

  it('renders one button per suggestion, under the input row', () => {
    const html = render({ suggestions: SUGGESTED });
    expect(html.match(/class="bar-suggest-chip"/g)).toHaveLength(3);
    expect(html).toContain('bar-suggest-head">Suggested:</span>');
    expect(html.indexOf('bar-field')).toBeLessThan(html.indexOf('bar-suggest"'));
    // After the send button in the markup, so it wraps onto its own line below the field.
    expect(html.indexOf('bar-send')).toBeLessThan(html.indexOf('bar-suggest"'));
  });

  it('each chip carries the full text as its title and label', () => {
    const html = render({ suggestions: ['Show the diff of the last change'] });
    const chip = tagHolding(html, 'button', 'bar-suggest-chip');
    expect(chip).toContain('data-tip="Show the diff of the last change" data-tip-clip=""');
    expect(chip).not.toContain('title=');
    expect(chip).toContain('aria-label="Show the diff of the last change"');
    expect(chip).toContain('type="button"');
  });

  it('a suggestion that starts with a slash is an ordinary chip, not a command', () => {
    const html = render({ suggestions: ['/help me pick a file'] });
    expect(html.match(/class="bar-suggest-chip"/g)).toHaveLength(1);
    expect(html).toContain('>/help me pick a file</button>');
  });

  it('is not rendered with no suggestions', () => {
    expect(render()).not.toContain('bar-suggest');
    expect(render({ suggestions: [] })).not.toContain('bar-suggest');
  });

  it('is hidden while the agent is busy, the bar is locked, or in the exited and collapsed modes', () => {
    expect(render({ suggestions: SUGGESTED, busy: true })).not.toContain('bar-suggest');
    expect(render({ suggestions: SUGGESTED, locked: true })).not.toContain('bar-suggest');
    expect(render({ suggestions: SUGGESTED, mode: 'exited' })).not.toContain('bar-suggest');
    expect(render({ suggestions: SUGGESTED, mode: 'collapsed' })).not.toContain('bar-suggest');
  });

  it('shows alongside a queue, and the queue row stays above the field', () => {
    const queue: QueuedMessage[] = [{ id: 1, text: 'check the tests', attachments: [] }];
    const html = render({ suggestions: SUGGESTED, queue, queueMode: 'all' });
    expect(html).toContain('bar-suggest"');
    expect(html.indexOf('bar-queue')).toBeLessThan(html.indexOf('bar-field'));
  });
});
