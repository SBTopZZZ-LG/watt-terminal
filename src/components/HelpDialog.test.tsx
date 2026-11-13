import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { HelpDialog } from './HelpDialog';
import type { HelpLoad } from './HelpDialog';

const NOOP = () => undefined;

const READY: HelpLoad = { kind: 'ready', text: 'Slash commands:\n/help — List the slash commands.' };

const html = (load: HelpLoad = READY) => renderToStaticMarkup(<HelpDialog load={load} onRetry={NOOP} onClose={NOOP} />);

describe('HelpDialog', () => {
  it('is a labelled modal in the shared backdrop, reusing the settings card with a help-dialog class', () => {
    const out = html();
    expect(out).toMatch(/<div class="modal-backdrop"><div class="settings-dialog help-dialog" role="dialog"/);
    expect(out).toContain('aria-modal="true"');
    expect(out).toMatch(/aria-labelledby="[^"]+"/);
  });

  it('is titled Slash commands, with the help text pre-wrapped and a Close button', () => {
    const out = html();
    expect(out).toContain('<h2 class="settings-title"');
    expect(out).toContain('>Slash commands</h2>');
    expect(out).toContain('<p class="help-text">Slash commands:\n/help — List the slash commands.</p>');
    expect(out).toMatch(/<button[^>]*>Close<\/button>/);
  });

  it('shows help text as text, never markup', () => {
    const out = html({ kind: 'ready', text: '/evil <img src=x> & <b>bold</b>' });
    expect(out).not.toContain('<img');
    expect(out).toContain('&lt;img src=x&gt;');
    expect(out).toContain('&lt;b&gt;');
  });

  it('loading shows a status line and no help text, with Close still there', () => {
    const out = html({ kind: 'loading' });
    expect(out).toContain('role="status"');
    expect(out).toContain('Loading slash commands...');
    expect(out).not.toContain('help-text');
    expect(out).toMatch(/<button[^>]*>Close<\/button>/);
  });

  it('a failed load shows the message as an alert, with Retry and Close', () => {
    const out = html({ kind: 'error', message: 'the store is gone' });
    expect(out).toContain('role="alert"');
    expect(out).toContain('Could not load the slash commands: the store is gone');
    expect(out).toMatch(/<button[^>]*>Retry<\/button>/);
    expect(out).toMatch(/<button[^>]*>Close<\/button>/);
  });

  it('the body scrolls: the text sits in a help-body between the head and the foot', () => {
    const out = html();
    const bodyAt = out.indexOf('class="help-body"');
    expect(bodyAt).toBeGreaterThan(out.indexOf('settings-head'));
    expect(bodyAt).toBeLessThan(out.indexOf('settings-foot'));
  });
});
