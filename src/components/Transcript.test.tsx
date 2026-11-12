import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { EMPTY_TRANSCRIPT, transcriptReducer } from '../lib/transcript';
import type { TranscriptAction } from '../lib/transcript';
import { JumpToLatest, Transcript } from './Transcript';

function render(actions: TranscriptAction[]): string {
  const t = actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT);
  return renderToStaticMarkup(
    <Transcript
      items={t.items}
      busy={false}
      thinking={false}
      open
      onToggle={() => {}}
      onApprove={() => {}}
      onNewConversation={() => {}}
    />,
  );
}

const delta = (text: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text_delta', text } });
const done: TranscriptAction = { type: 'agent_event', event: { type: 'done' } };
const text = (t: string): TranscriptAction => ({ type: 'agent_event', event: { type: 'text', text: t } });

const COPY = 'aria-label="Copy as Markdown"';

describe('Transcript: copy button', () => {
  it('is not shown while the agent message is still streaming', () => {
    const html = render([delta('partial reply')]);
    expect(html).toContain('partial reply');
    expect(html).not.toContain(COPY);
  });

  it('is shown once the streamed message is complete', () => {
    const html = render([delta('full '), delta('reply'), done]);
    expect(html).toContain(COPY);
    expect(html).toContain('title="Copy as Markdown"');
  });

  it('is shown on a message that arrived as one text event', () => {
    const html = render([text('one shot')]);
    expect(html.split(COPY)).toHaveLength(2);
  });

  it('is not shown on user messages', () => {
    const html = render([{ type: 'user_sent', clientId: 1, text: 'hello' }]);
    expect(html).not.toContain(COPY);
  });
});

describe('Transcript: jump to latest', () => {
  // The scroll behaviour needs a DOM (no jsdom here), so only the initial render is checked: the button starts hidden.
  it('is not shown on the initial render, even with a long conversation', () => {
    const html = render([delta('a'.repeat(50)), done, text('more'), { type: 'user_sent', clientId: 2, text: 'next' }]);
    expect(html).not.toContain('jump-to-latest');
  });

  it('says Scroll to bottom when the agent is idle', () => {
    const html = renderToStaticMarkup(<JumpToLatest busy={false} onClick={() => {}} />);
    expect(html).toContain('aria-label="Scroll to bottom"');
    expect(html).toContain('type="button"');
  });

  it('says Follow the agent while it is busy', () => {
    const html = renderToStaticMarkup(<JumpToLatest busy onClick={() => {}} />);
    expect(html).toContain('aria-label="Follow the agent (resume auto-scroll)"');
  });
});

describe('Transcript: tool groups', () => {
  it('a single tool call with a resolved approval renders as a group, with its rows inside the indented block', () => {
    const html = render([
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'c1', name: 'run_command', args: {} } },
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'run_command', text: 'ls' } },
      {
        type: 'agent_event',
        event: { type: 'tool_result', call_id: 'c1', name: 'run_command', ok: true, decision: 'manual', result: {} },
      },
    ]);
    expect(html).toContain('1 tool call: run_command');
    expect(html).toContain('class="tool-group-items"');
    expect(html.indexOf('class="tool-group-items"')).toBeLessThan(html.indexOf('class="approval approval-resolved"'));
  });
});

describe('Transcript: history button', () => {
  it('has the id App focuses when the History view closes, and is pressed while the view is shown', () => {
    const html = renderToStaticMarkup(
      <Transcript
        items={[]}
        busy={false}
        thinking={false}
        open
        onToggle={() => {}}
        onApprove={() => {}}
        onNewConversation={() => {}}
        view="history"
        onToggleHistory={() => {}}
        history={<div>history</div>}
      />,
    );
    expect(html).toContain('id="history-button"');
    expect(html).toMatch(/id="history-button"[^>]*aria-pressed="true"|aria-pressed="true"[^>]*id="history-button"/);
  });
});

describe('Transcript: todo panel', () => {
  const todos = [{ content: 'plan', status: 'in_progress' as const }];
  const base = { items: [], busy: false, thinking: false, onToggle: () => {}, onApprove: () => {}, onNewConversation: () => {} };

  it('is pinned above the scrolling body, not inside it', () => {
    const html = renderToStaticMarkup(<Transcript {...base} todos={todos} open />);
    const panel = html.indexOf('class="todo-panel');
    expect(panel).toBeGreaterThan(-1);
    expect(panel).toBeLessThan(html.indexOf('class="transcript-body"'));
  });

  it('is hidden when the list is empty, and when the chat body is not shown', () => {
    expect(renderToStaticMarkup(<Transcript {...base} todos={[]} open />)).not.toContain('todo-panel');
    expect(renderToStaticMarkup(<Transcript {...base} todos={todos} open={false} />)).not.toContain('todo-panel');
    expect(renderToStaticMarkup(<Transcript {...base} todos={todos} open view="history" history={<div />} />)).not.toContain('todo-panel');
  });

  it('todo_write rows never render as tool rows', () => {
    // Run through the reducer, as the view gets them: the reducer drops the call and its result.
    const t = [
      { type: 'user_sent' as const, clientId: 1, text: 'plan' },
      { type: 'agent_event' as const, event: { type: 'tool_call' as const, call_id: 't1', name: 'todo_write', args: {} } },
      { type: 'agent_event' as const, event: { type: 'tool_result' as const, call_id: 't1', name: 'todo_write', ok: true, decision: 'read' as const, result: {} } },
    ].reduce(transcriptReducer, EMPTY_TRANSCRIPT);
    const html = renderToStaticMarkup(<Transcript {...base} items={t.items} open />);
    expect(html).toContain('plan');
    expect(html).not.toContain('todo_write');
    expect(html).not.toContain('class="tool');
  });
});

describe('Transcript: approval reasons', () => {
  const callAndResult = (decision: 'auto' | 'manual' | 'denied', reason?: 'rule' | 'gate_chars') => [
    { type: 'agent_event' as const, event: { type: 'tool_call' as const, call_id: 'c1', name: 'run_command', args: {} } },
    {
      type: 'agent_event' as const,
      event: { type: 'tool_result' as const, call_id: 'c1', name: 'run_command', ok: decision !== 'denied', decision, ...(reason ? { reason } : {}), result: {} },
    },
  ];

  it('an automatic run shows its reason in the label, and why in the tooltip', () => {
    const html = render(callAndResult('auto', 'rule'));
    expect(html).toContain('auto · rule');
    expect(html).toContain('title="Ran without asking: an allow rule matches this command."');
  });

  it('an approved call keeps the label "manual", and its tooltip says why it asked', () => {
    const html = render(callAndResult('manual', 'gate_chars'));
    expect(html).toContain('decision-manual');
    expect(html).not.toContain('manual · ');
    expect(html).toContain('title="Asked because the command has a character outside letters, digits, spaces and _-./:=@+, so your rules do not apply to it."');
  });

  it('a call with no reason has the bare label and no tooltip', () => {
    const html = render(callAndResult('auto'));
    expect(html).toContain('decision-auto');
    expect(html).not.toContain('auto · ');
    expect(html).not.toContain('title="Ran without');
  });

  it('the approval card says why it asks, in one muted line', () => {
    const html = render([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'run_command', text: 'ls | wc', reason: 'gate_chars' } },
    ]);
    expect(html).toContain('<div class="approval-reason">Asked because the command has a character outside');
  });

  it('an approval card without a reason has no reason line', () => {
    const html = render([{ type: 'agent_event', event: { type: 'approval_request', call_id: 'c1', name: 'send_keys', text: 'ls' } }]);
    expect(html).toContain('class="approval approval-pending"');
    expect(html).not.toContain('approval-reason');
  });
});

describe('Transcript: a request_integration row', () => {
  const callIntegrate: TranscriptAction = {
    type: 'agent_event',
    event: { type: 'tool_call', call_id: 'c1', name: 'request_integration', args: {} },
  };

  it('a declined request is a tool row with the status declined, not failed and not denied', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: false, decision: 'denied', result: 'declined: the user did not integrate this shell' } },
    ]);
    expect(html).toContain('class="tool tool-declined"');
    expect(html).toContain('request_integration');
    expect(html).toContain(': declined</summary>');
    expect(html).not.toContain('decision-denied');
    expect(html).not.toContain('tool-failed');
  });

  it('an integrated request is a plain ok row, like the other tool rows', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: true, decision: 'manual', result: 'Integrated ssh ubuntu@lidia (zsh).' } },
    ]);
    expect(html).toContain('class="tool tool-ok"');
    expect(html).toContain('decision decision-manual');
    expect(html).toContain(': ok</summary>');
  });

  it('a request run without a dialog (auto) shows auto, like any other tool', () => {
    const html = render([
      callIntegrate,
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'c1', name: 'request_integration', ok: true, decision: 'auto', result: 'x' } },
    ]);
    expect(html).toContain('auto');
  });
});

describe('Transcript: interactive command groups', () => {
  const started = (at: number): TranscriptAction[] => [
    { type: 'agent_event', event: { type: 'tool_call', call_id: 'rc', name: 'run_command', args: { command: 'ssh ubuntu@lidia' } }, at },
    {
      type: 'agent_event',
      event: { type: 'tool_result', call_id: 'rc', name: 'run_command', ok: true, decision: 'auto', result: { outcome: 'running', record_id: 7 } },
    },
  ];
  const wait: TranscriptAction[] = [
    { type: 'agent_event', event: { type: 'tool_call', call_id: 'w1', name: 'wait', args: {} } },
    { type: 'agent_event', event: { type: 'tool_result', call_id: 'w1', name: 'wait', ok: true, decision: 'read', result: {} } },
  ];
  const endOf7: TranscriptAction = {
    type: 'record_finished',
    record: { id: 7, command: 'ssh ubuntu@lidia', source: { kind: 'agent', call_id: 'rc' }, exit_code: 0, status: 'complete', used_alt_screen: false, output_bytes: 0 },
  };

  it('a running command is a wrapper with the accent rail and a header that says running', () => {
    const html = render([...started(Date.now()), ...wait]);
    expect(html).toContain('class="interactive interactive-running"');
    expect(html).toMatch(/<span class="interactive-state" aria-hidden="true">Interactive · running \d+s<\/span>/);
    expect(html).toContain('<span class="interactive-command" aria-hidden="true"> · ssh ubuntu@lidia</span>');
    expect(html).toMatch(/<div class="interactive-header" role="group" aria-label="Interactive · running \d+s · ssh ubuntu@lidia">/);
    expect(html).toContain('class="interactive-items"');
    // The run_command row is the header, not a row inside the group.
    expect(html).not.toContain('>run_command');
  });

  it('a finished command has no rail and its header says finished with the exit code', () => {
    const html = render([...started(Date.now()), ...wait, endOf7]);
    expect(html).toContain('class="interactive interactive-ended"');
    expect(html).not.toContain('interactive-running');
    expect(html).toContain('<span class="interactive-state" aria-hidden="true">Interactive · finished · exit 0</span>');
    expect(html).toContain('aria-label="Interactive · finished · exit 0 · ssh ubuntu@lidia"');
  });

  it('an item after the end is outside the wrapper', () => {
    const html = render([...started(Date.now()), endOf7, { type: 'agent_event', event: { type: 'text', text: 'after the end' } }]);
    const wrapperEnd = html.indexOf('interactive-items');
    expect(wrapperEnd).toBeGreaterThan(-1);
    expect(html.indexOf('after the end')).toBeGreaterThan(html.indexOf('interactive-ended'));
    expect(html.slice(wrapperEnd).indexOf('after the end')).toBeGreaterThan(-1);
  });

  it('a record_end with no group shows nothing', () => {
    const html = render([endOf7]);
    expect(html).not.toContain('interactive');
    expect(html).not.toContain('record');
  });

  it('a send_keys row shows what it typed, and its approval card the decoded keys in a block', () => {
    const html = render([
      { type: 'agent_event', event: { type: 'approval_request', call_id: 'k1', name: 'send_keys', text: 'echo hi⏎', display: 'echo hi⏎', reason: 'unintegrated_shell' } },
      { type: 'agent_event', event: { type: 'tool_call', call_id: 'k1', name: 'send_keys', args: { text: 'echo hi', keys: ['Enter'] } } },
      { type: 'agent_event', event: { type: 'tool_result', call_id: 'k1', name: 'send_keys', ok: true, decision: 'manual', reason: 'unintegrated_shell', result: {} } },
    ]);
    expect(html).toContain('The agent wants to type into a running command');
    expect(html).toContain('<code class="approval-text approval-keys">echo hi⏎</code>');
    expect(html).toContain('Typed into a shell without integration: your rules cannot check this.');
    expect(html).toContain('echo hi⏎');
    expect(html).toContain('send_keys');
  });

  it('a long typed text is cut in the row summary', () => {
    const long = 'x'.repeat(80);
    const html = render([{ type: 'agent_event', event: { type: 'tool_call', call_id: 'k2', name: 'send_keys', args: { text: long } } }]);
    // The summary is cut; the full text stays in the row's details (its <pre>).
    const summary = /<summary>([^]*?)<\/summary>/.exec(html)?.[1] ?? '';
    expect(summary).toContain(`${'x'.repeat(39)}…`);
    expect(summary).not.toContain('x'.repeat(40));
    expect(html).toContain('x'.repeat(80));
  });
});

describe('Transcript: attachments on a message the user sent', () => {
  const THUMB = 'data:image/jpeg;base64,/9j/AAAA';

  it('a live message shows its image as a thumbnail and its document as a chip with name and size, under the text', () => {
    const html = render([
      {
        type: 'user_sent',
        clientId: 1,
        text: 'what is in these',
        attachments: [
          { kind: 'image', name: 'shot.png', bytes: 2048, thumb: THUMB },
          { kind: 'file', name: 'notes.md', bytes: 300 },
        ],
      },
    ]);
    expect(html).toContain('<div class="msg-attachments">');
    expect(html).toContain(`<img class="msg-attach-thumb" src="${THUMB}" alt="shot.png" title="shot.png"/>`);
    expect(html).toContain('<span class="msg-attach-name" title="notes.md">notes.md</span><span class="msg-attach-size">300 B</span>');
    // The text comes first, then the attachments.
    expect(html.indexOf('what is in these')).toBeLessThan(html.indexOf('msg-attachments'));
  });

  it('an attachment-only message renders with no text, and no empty text row is needed', () => {
    const html = render([{ type: 'user_sent', clientId: 2, text: '', attachments: [{ kind: 'file', name: 'only.md', bytes: 4096 }] }]);
    expect(html).toContain('<span class="msg-attach-name" title="only.md">only.md</span><span class="msg-attach-size">4 KB</span>');
  });

  it('an image with no stored thumbnail is a chip with its name, not an image', () => {
    const html = render([{ type: 'user_sent', clientId: 3, text: 'x', attachments: [{ kind: 'image', name: 'p.jpg', bytes: 900 }] }]);
    expect(html).toContain('class="msg-attach is-placeholder"');
    expect(html).toContain('<span class="msg-attach-name" title="p.jpg">p.jpg</span>');
    expect(html).not.toContain('msg-attach-thumb');
  });

  it('names are shown as text: markup is escaped and bidi overrides are removed', () => {
    const html = render([
      { type: 'user_sent', clientId: 4, text: 'x', attachments: [{ kind: 'file', name: '<img src=x onerror=alert(1)>‮evil.txt', bytes: 1 }] },
    ]);
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;evil.txt');
    expect(html).not.toContain('<img src=x');
    expect(html).not.toContain('‮');
  });

  it('a message with no attachments has no attachment row', () => {
    expect(render([{ type: 'user_sent', clientId: 5, text: 'plain' }])).not.toContain('msg-attachments');
  });
});

describe('Transcript: the trim note', () => {
  const usage = (trimmed: boolean): TranscriptAction => ({
    type: 'agent_event',
    event: { type: 'context_usage', limit: 8000, used: 100, reserved_output: 4096, trimmed, calibrated: false, categories: [] },
  });

  it('is a muted notice, not a warning', () => {
    const html = render([{ type: 'user_sent', clientId: 1, text: 'hi' }, usage(true)]);
    expect(html).toContain('<div class="notice notice-muted">Older messages or tool output were left out of the request');
  });

  it('a request that was not trimmed shows no note, and no meter markup', () => {
    const html = render([{ type: 'user_sent', clientId: 1, text: 'hi' }, usage(false)]);
    expect(html).not.toContain('notice');
    expect(html).not.toContain('context-meter');
  });
});
