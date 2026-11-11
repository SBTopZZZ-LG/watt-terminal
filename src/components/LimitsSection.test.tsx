import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { LimitsSection, UNLIMITED_WARNING } from './LimitsSection';
import type { LimitsSectionProps } from './LimitsSection';
import { draftFromLimits } from '../lib/limitsForm';
import type { LimitsSettings } from '../lib/types';

const NOOP = () => undefined;
const SAVED: LimitsSettings = { max_model_calls: 50, model_calls_unlimited: false, max_poll_calls: 60, max_commands_per_turn: 30, max_wait_minutes: 30 };

const BASE: LimitsSectionProps = {
  draft: draftFromLimits(SAVED),
  dirty: false,
  locked: false,
  saving: false,
  saveError: null,
  onChange: NOOP,
  onSave: NOOP,
  onReset: NOOP,
};

const render = (over: Partial<LimitsSectionProps> = {}) => renderToStaticMarkup(<LimitsSection {...BASE} {...over} />);

/** The opening tag of the element that holds `needle`: searches back from the needle to the nearest `<tag`. */
function tagHolding(html: string, tag: string, needle: string): string {
  const at = html.indexOf(needle);
  expect(at, `${needle} is in the markup`).toBeGreaterThan(-1);
  const start = html.lastIndexOf(`<${tag}`, at);
  const end = html.indexOf('>', start);
  return html.slice(start, end + 1);
}

describe('LimitsSection: fields', () => {
  it('has the four fields, each with its label and its default as the placeholder', () => {
    const html = render();
    expect(html).toContain('>Model calls per turn</label>');
    expect(html).toContain('>Polling calls per turn</label>');
    expect(html).toContain('>Commands per turn</label>');
    expect(html).toContain('>Wait time per turn (minutes)</label>');
    expect(tagHolding(html, 'input', 'default 50')).toContain('value="50"');
    expect(tagHolding(html, 'input', 'default 60')).toContain('value="60"');
    expect(tagHolding(html, 'input', 'default 30')).toContain('value="30"');
    expect(html.match(/placeholder="default 30"/g)?.length).toBe(2);
  });

  it('gives the help text of each field, with the range', () => {
    const html = render();
    expect(html).toContain('A turn stops after this many model replies that are not polling. Default 50, at least 10.');
    expect(html).toContain('wait, get_screen and send_keys while a command you started is running are counted here, separately. Default 60, 10 to 500.');
    expect(html).toContain('Auto-approved commands are still capped at 8 per turn by the approval rules. Default 30, 1 to 500.');
    expect(html).toContain('Total time the agent may spend in wait calls in one turn. Default 30, 1 to 720.');
  });

  it('says a change applies from the next message', () => {
    expect(render()).toContain('A change applies from the next message.');
  });

  it('shows a Reset to defaults button, enabled', () => {
    expect(tagHolding(render(), 'button', 'Reset to defaults')).not.toContain('disabled=""');
  });
});

describe('LimitsSection: save', () => {
  it('Save limits is disabled while the draft is clean', () => {
    expect(tagHolding(render(), 'button', 'Save limits')).toContain('disabled=""');
    expect(render()).not.toContain('settings-unsaved');
  });

  it('a valid dirty draft shows the unsaved marker and enables Save limits', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), pollCalls: '120' } });
    expect(html).toContain('settings-unsaved">Unsaved changes</span>');
    expect(tagHolding(html, 'button', 'Save limits')).not.toContain('disabled=""');
  });

  it('an invalid draft disables Save and names the field that is out of range, inline', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), pollCalls: '9' } });
    expect(tagHolding(html, 'button', 'Save limits')).toContain('disabled=""');
    expect(html).toContain('<span class="settings-error" id=');
    expect(html).toContain('Enter a whole number from 10 to 500.</span>');
    expect(tagHolding(html, 'input', 'value="9"')).toContain('aria-invalid="true"');
  });

  it('a model-call number that is not a whole number is named and blocks the save', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), modelCalls: '2.5' } });
    expect(html).toContain('Enter a whole number from 10 to 4294967295.</span>');
    expect(tagHolding(html, 'button', 'Save limits')).toContain('disabled=""');
  });

  it('shows Saving... and freezes every field while a save is in flight', () => {
    const html = render({ dirty: true, saving: true });
    expect(html).toContain('Saving...');
    expect(tagHolding(html, 'input', 'default 60')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Reset to defaults')).toContain('disabled=""');
  });

  it('shows the backend error in an alert and keeps the draft', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), pollCalls: '500' }, saveError: 'limits: polling calls per turn must be 10 to 500' });
    expect(html).toContain('role="alert">limits: polling calls per turn must be 10 to 500</p>');
    expect(tagHolding(html, 'input', 'value="500"')).toContain('value="500"');
  });
});

describe('LimitsSection: unlimited model calls', () => {
  it('the checkbox is clear and the number enabled when the cap is in effect', () => {
    const html = render();
    expect(tagHolding(html, 'input', 'type="checkbox"')).not.toContain('checked=""');
    expect(tagHolding(html, 'input', 'value="50"')).not.toContain('disabled=""');
  });

  it('unlimited on: the box is checked, the number is disabled but still shown', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), unlimited: true } });
    expect(tagHolding(html, 'input', 'type="checkbox"')).toContain('checked=""');
    expect(tagHolding(html, 'input', 'value="50"')).toContain('disabled=""');
  });

  it('the warning for a high number is not shown while unlimited is on, since the cap does not apply', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), modelCalls: '500', unlimited: true } });
    expect(html).not.toContain('A high limit lets a runaway turn cost a lot.');
  });

  it('the unlimited warning text is the one asked for', () => {
    expect(UNLIMITED_WARNING).toBe(
      'The agent can then keep making model calls until you cancel it. Every call re-sends the conversation, so a runaway turn can cost a lot.',
    );
  });

  it('the confirm is not in the markup until unlimited is asked for', () => {
    expect(render()).not.toContain('role="alertdialog"');
  });
});

describe('LimitsSection: high number warning', () => {
  it('warns above 200, under the number', () => {
    const html = render({ dirty: true, draft: { ...draftFromLimits(SAVED), modelCalls: '250' } });
    expect(html).toContain('<span class="settings-warn" id=');
    expect(html).toContain('A high limit lets a runaway turn cost a lot.</span>');
  });

  it('does not warn at 200', () => {
    expect(render({ draft: { ...draftFromLimits(SAVED), modelCalls: '200' } })).not.toContain('A high limit lets a runaway turn cost a lot.');
  });
});

describe('LimitsSection: locked', () => {
  it('locks every field and both buttons when settings.json cannot be used', () => {
    const html = render({ dirty: true, locked: true });
    expect(tagHolding(html, 'input', 'default 60')).toContain('disabled=""');
    expect(tagHolding(html, 'input', 'default 30')).toContain('disabled=""');
    expect(tagHolding(html, 'input', 'type="checkbox"')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Save limits')).toContain('disabled=""');
    expect(tagHolding(html, 'button', 'Reset to defaults')).toContain('disabled=""');
  });
});
