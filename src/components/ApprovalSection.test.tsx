import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ApprovalSection } from './ApprovalSection';
import type { ApprovalSectionProps } from './ApprovalSection';

const NOOP = () => undefined;

const BASE: ApprovalSectionProps = {
  draft: { mode: 'regex', allow: 'git status\nls .*', deny: 'rm .*', nested: 'ask' },
  dirty: false,
  locked: false,
  saving: false,
  saveError: null,
  builtinDeny: ['rm', 'sudo'],
  integration: 'ask',
  integrationBusy: false,
  integrationError: null,
  onIntegrationChange: NOOP,
  onChange: NOOP,
  onSave: NOOP,
  onUndo: NOOP,
};

const render = (over: Partial<ApprovalSectionProps> = {}) => renderToStaticMarkup(<ApprovalSection {...BASE} {...over} />);

/** The opening tag of the element that holds `needle`: searches back from the needle to the nearest `<tag`. */
function tagHolding(html: string, tag: string, needle: string): string {
  const at = html.indexOf(needle);
  expect(at, `${needle} is in the markup`).toBeGreaterThan(-1);
  const start = html.lastIndexOf(`<${tag}`, at);
  const end = html.indexOf('>', start);
  return html.slice(start, end + 1);
}

describe('ApprovalSection: mode', () => {
  it('shows the saved mode as the checked radio', () => {
    const html = render({ draft: { ...BASE.draft, mode: 'regex' } });
    expect(tagHolding(html, 'input', 'value="regex"')).toContain('checked=""');
    expect(tagHolding(html, 'input', 'value="manual"')).not.toContain('checked=""');
  });

  it('manual is checked when the draft says manual', () => {
    const html = render({ draft: { ...BASE.draft, mode: 'manual' } });
    expect(tagHolding(html, 'input', 'value="manual"')).toContain('checked=""');
    expect(tagHolding(html, 'input', 'value="regex"')).not.toContain('checked=""');
  });

  it('labels the two modes Manual and Regex rules, in a Mode group', () => {
    const html = render();
    expect(html).toContain('<legend>Mode</legend>');
    expect(html).toContain('Manual');
    expect(html).toContain('Regex rules');
  });
});

describe('ApprovalSection: rules', () => {
  it('shows each list in its box, one rule per line', () => {
    const html = render();
    expect(tagHolding(html, 'textarea', 'git status')).toContain('aria-describedby=');
    expect(html).toContain('git status\nls .*</textarea>');
    expect(html).toContain('rm .*</textarea>');
  });

  it('shows the count and the limit under each box', () => {
    const html = render();
    expect(html).toContain('2 of 50 rules');
    expect(html).toContain('1 of 50 rules');
  });

  it('marks a box over the limit as invalid', () => {
    const many = Array.from({ length: 51 }, (_, i) => `cmd${i}`).join('\n');
    const html = render({ draft: { ...BASE.draft, allow: many } });
    expect(tagHolding(html, 'textarea', 'cmd0')).toContain('aria-invalid="true"');
    expect(render()).not.toContain('aria-invalid');
  });

  it('keeps the note on what can run by rule and on approve-all', () => {
    const html = render();
    expect(html).toContain('letters, digits, spaces and the characters _-./:=@+,');
    expect(html).toContain('Approve-all in the status bar overrides the rules for the session.');
    expect(html).toContain('At most 8 commands run this way in one turn');
  });
});

describe('ApprovalSection: save and undo', () => {
  it('Save and Undo are disabled while the draft is clean', () => {
    const html = render();
    expect(tagHolding(html, 'button', 'Save approvals')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Undo changes')).toContain('disabled=""');
    expect(html).not.toContain('settings-unsaved');
  });

  it('a dirty draft shows the unsaved marker and enables Save and Undo', () => {
    const html = render({ dirty: true });
    expect(html).toContain('settings-unsaved">Unsaved changes</span>');
    expect(tagHolding(html, 'button', 'Save approvals')).not.toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Undo changes')).not.toContain('disabled=""');
  });

  it('shows Saving... and freezes the controls while a save is in flight', () => {
    const html = render({ dirty: true, saving: true });
    expect(html).toContain('Saving...');
    expect(tagHolding(html, 'textarea', 'git status')).toContain('disabled=""');
    expect(tagHolding(html, 'input', 'value="manual"')).toContain('disabled=""');
  });

  it('shows the backend error in an alert, next to the buttons', () => {
    const html = render({ dirty: true, saveError: 'approval allow rule 2 is not a valid pattern' });
    expect(html).toContain('role="alert">approval allow rule 2 is not a valid pattern</p>');
  });
});

describe('ApprovalSection: locked', () => {
  it('locks every control when settings.json cannot be used', () => {
    const html = render({ dirty: true, locked: true });
    expect(tagHolding(html, 'input', 'value="regex"')).toContain('disabled=""');
    expect(tagHolding(html, 'textarea', 'git status')).toContain('disabled=""');
    expect(tagHolding(html, 'textarea', 'rm .*')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Save approvals')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Undo changes')).toContain('disabled=""');
    expect(tagHolding(html, 'input', 'type="text"')).toContain('disabled=""');
    expect(tagHolding(html, 'button', '>Check</button>')).toContain('disabled=""');
  });
});

describe('ApprovalSection: built-in deny list', () => {
  it('is a collapsed details with the count and the words', () => {
    const html = render();
    expect(html).toContain('<details class="settings-builtin">');
    expect(html).toContain('Built-in deny list (2 commands)');
    expect(html).toContain('rm, sudo');
    expect(html).toContain('These first words always ask, whatever the rules say.');
  });

  it('says it is loading while the list has not arrived', () => {
    const html = render({ builtinDeny: undefined });
    expect(html).toContain('Built-in deny list</summary>');
    expect(html).not.toContain('settings-builtin-list');
  });

  it('says it is not available when it could not be read', () => {
    const html = render({ builtinDeny: null });
    expect(html).toContain('Built-in deny list (not available)');
    expect(html).not.toContain('settings-builtin-list');
  });
});

describe('ApprovalSection: try a command', () => {
  it('says it checks the saved settings, not the draft', () => {
    const html = render();
    expect(html).toContain('Checks the saved mode and rules. Unsaved changes above are not used, and nothing runs.');
    expect(html).toContain('>Try a command</label>');
  });

  it('starts with Check disabled, since there is no command yet', () => {
    expect(tagHolding(render(), 'button', '>Check</button>')).toContain('disabled=""');
  });
});

describe('ApprovalSection: Remote shells', () => {
  it('has a Remote shells block with the two controls', () => {
    const html = render();
    expect(html).toContain('Remote shells');
    expect(html).toContain('Auto-approve in SSH and nested shells');
    expect(html).toContain('Let the agent integrate shells without asking');
  });

  it('the SSH and nested box is checked for same_as_local, and not otherwise', () => {
    const on = render({ draft: { ...BASE.draft, nested: 'same_as_local' } });
    expect(tagHolding(on, 'input', 'Auto-approve in SSH and nested shells')).toContain('checked=""');
    const off = render();
    expect(tagHolding(off, 'input', 'Auto-approve in SSH and nested shells')).not.toContain('checked=""');
  });

  it('the agent-request box is checked for auto only', () => {
    const auto = render({ integration: 'auto' });
    expect(tagHolding(auto, 'input', 'Let the agent integrate shells without asking')).toContain('checked=""');
    const ask = render({ integration: 'ask' });
    expect(tagHolding(ask, 'input', 'Let the agent integrate shells without asking')).not.toContain('checked=""');
  });

  it('carries the warnings: remote aliases, PATH and users; the hard gate and deny rules still apply; the loader and the prompts', () => {
    const html = render();
    expect(html).toContain('Remote aliases, PATH and users differ from this machine');
    expect(html).toContain('The hard gate and the deny rules still apply everywhere.');
    expect(html).toContain('Applies when you save approvals.');
    expect(html).toContain('the agent can type the integration loader into whatever shell is in');
    expect(html).toContain('It refuses at password and other prompts.');
  });

  it('the agent-request box is disabled while it saves, and the nested box while the draft is frozen', () => {
    const saving = render({ integrationBusy: true });
    expect(tagHolding(saving, 'input', 'Let the agent integrate shells without asking')).toContain('disabled=""');
    expect(saving).toContain('Saving...');
    const locked = render({ locked: true });
    expect(tagHolding(locked, 'input', 'Auto-approve in SSH and nested shells')).toContain('disabled=""');
    expect(tagHolding(locked, 'input', 'Let the agent integrate shells without asking')).toContain('disabled=""');
  });

  it('shows a refused integration save as an alert', () => {
    const html = render({ integrationError: 'settings.json is broken; fix or revert it first' });
    expect(html).toContain('role="alert"');
    expect(html).toContain('settings.json is broken; fix or revert it first');
  });
});
