import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ApprovalToast } from './ApprovalToast';
import type { Dock } from '../lib/panelLayout';
import { DOCKS } from '../lib/panelLayout';
import type { TranscriptItem } from '../lib/transcript';

type ApprovalItem = Extract<TranscriptItem, { kind: 'approval' }>;

const NOOP = () => undefined;

const RUN: ApprovalItem = {
  id: 1,
  kind: 'approval',
  callId: 'c1',
  name: 'run_command',
  text: 'git status',
  display: null,
  state: 'pending',
  approve: null,
  reason: null,
  location: null,
};

const render = (approval: ApprovalItem = RUN, dock: Dock = 'right') =>
  renderToStaticMarkup(<ApprovalToast approval={approval} dock={dock} onApprove={NOOP} onOpen={NOOP} />);

describe('ApprovalToast', () => {
  it('is an alert dialog with the approval title and the dock class', () => {
    const html = render();
    expect(html).toContain('class="approval-toast dock-right"');
    expect(html).toContain('role="alertdialog"');
    expect(html).toContain('aria-label="Approval needed"');
    expect(html).toContain('Approval needed');
  });

  it('carries the dock class for every dock', () => {
    for (const dock of DOCKS) {
      expect(render(RUN, dock as Dock)).toContain(`class="approval-toast dock-${dock}"`);
    }
  });

  it('shows a run_command as escaped text with Approve, Deny and Show buttons', () => {
    const html = render();
    expect(html).toContain('<code class="approval-toast-command">git status</code>');
    expect(html).toContain('aria-label="Approve">Approve</button>');
    expect(html).toContain('aria-label="Deny">Deny</button>');
    expect(html).toContain('aria-label="Show the approval">Show</button>');
  });

  it('truncates a long command with an ellipsis', () => {
    const html = render({ ...RUN, text: `git log ${'x'.repeat(200)}` });
    expect(html).toContain('…');
    expect(html).not.toContain('x'.repeat(200));
  });

  it('escapes hidden characters in the command', () => {
    const html = render({ ...RUN, text: 'rm \u202eevil' });
    expect(html).toContain('rm \\u{202E}evil');
    expect(html).not.toContain('\u202e');
  });

  it('shows a send_keys card as the decoded, escaped input', () => {
    const html = render({ ...RUN, name: 'send_keys', text: 'ls', display: 'echo hi⏎' });
    expect(html).toContain('<code class="approval-toast-command">echo hi⏎</code>');
  });

  it('escapes the decoded send_keys input', () => {
    const html = render({ ...RUN, name: 'send_keys', text: 'x', display: 'a‮b⏎' });
    expect(html).toContain('a\\u{202E}b⏎');
  });

  it('shows the countdown line when the approval has an expiry', () => {
    const html = render({ ...RUN, expiresAt: Date.now() + 120_000 });
    expect(html).toContain('approval-countdown');
    expect(html).toMatch(/expires in \d+:\d\d/);
  });

  it('has no countdown line for a card from an older session', () => {
    expect(render()).not.toContain('approval-countdown');
  });
});
