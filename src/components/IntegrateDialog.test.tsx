import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { IntegrateDialog } from './IntegrateDialog';
import type { OpenPhase } from '../lib/integrate';

const NOOP = () => undefined;

function dialog(phase: OpenPhase) {
  return renderToStaticMarkup(<IntegrateDialog phase={phase} onStart={NOOP} onClose={NOOP} />);
}

describe('IntegrateDialog: the consent screen', () => {
  const html = dialog({ kind: 'consent' });

  it('asks the question, as a labelled modal dialog', () => {
    expect(html).toContain('Integrate this shell?');
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toMatch(/aria-labelledby="[^"]+"/);
  });

  it('says what it does, and what it does not do', () => {
    expect(html).toContain('types one short command into the shell you are connected to');
    expect(html).toContain('about 4 KB');
    expect(html).toContain('Nothing is written to disk on the remote machine.');
    expect(html).toContain('ends when you exit it');
  });

  it('warns about prompts, names the shells, and says the loader may stay in zsh history', () => {
    expect(html).toContain('Make sure the remote shell is at its prompt, not at a password or other prompt: it will receive the typed lines.');
    expect(html).toContain('Works with zsh, and bash 4.4 or newer. Other shells are left untouched.');
    expect(html).toContain('The one-line loader may remain in zsh history (it is removed from bash history).');
  });

  it('has Integrate (primary) and Cancel', () => {
    expect(html).toMatch(/<button[^>]*class="integrate-primary"[^>]*>Integrate<\/button>/);
    expect(html).toMatch(/<button[^>]*>Cancel<\/button>/);
  });
});

describe('IntegrateDialog: running', () => {
  const html = dialog({ kind: 'running' });

  it('shows Integrating… as a status, with no buttons', () => {
    expect(html).toContain('Integrating…');
    expect(html).toContain('role="status"');
    expect(html).not.toContain('<button');
    expect(html).not.toContain('<footer');
  });
});

describe('IntegrateDialog: the result', () => {
  it('an integrated result is shown as ok, with a Close button', () => {
    const html = dialog({
      kind: 'result',
      outcome: { outcome: 'integrated', key: 1, label: 'ssh ubuntu@lidia', sh: 'zsh', user: 'ubuntu', host: 'lidia' },
    });
    expect(html).toContain('Integrated: ssh ubuntu@lidia (zsh as ubuntu@lidia). The agent can now run commands here.');
    expect(html).toContain('integrate-result is-ok');
    expect(html).toMatch(/<button[^>]*>Close<\/button>/);
    expect(html).not.toContain('>Integrate<');
  });

  it('a failure is shown as fail, with the reason in plain text', () => {
    const html = dialog({ kind: 'result', outcome: { outcome: 'failed', hint: 'agentterm: Unsupported shell (promptvars off)' } });
    expect(html).toContain('integrate-result is-fail');
    expect(html).toContain('Integration did not complete: agentterm: Unsupported shell (promptvars off).');
  });

  it('a refusal says why, with the same Close button', () => {
    const html = dialog({ kind: 'result', outcome: { outcome: 'refused', reason: 'alt_screen' } });
    expect(html).toContain('A full-screen program is open. Close it first, then try again.');
    expect(html).toMatch(/<button[^>]*>Close<\/button>/);
  });

  it('escapes a hint from the shell: it is text, never markup', () => {
    const html = dialog({ kind: 'result', outcome: { outcome: 'failed', hint: '<img src=x onerror=alert(1)>' } });
    expect(html).not.toContain('<img');
    expect(html).toContain('&lt;img');
  });
});
