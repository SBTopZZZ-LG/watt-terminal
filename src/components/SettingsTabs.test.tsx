import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SettingsTabs } from './SettingsTabs';
import type { SettingsTabsProps } from './SettingsTabs';

const TABS = [
  { id: 'models', label: 'Models' },
  { id: 'approvals', label: 'Approvals' },
];

function html(over: Partial<SettingsTabsProps> = {}): string {
  const p: SettingsTabsProps = {
    tabs: TABS,
    selected: 'models',
    dirty: [],
    label: 'AI settings sections',
    onSelect: () => undefined,
    panel: (id) => <p>{`PANEL-${id}`}</p>,
    ...over,
  };
  return renderToStaticMarkup(<SettingsTabs {...p} />);
}

/** Each tab button's markup, in order. */
function tabs(out: string): string[] {
  return out
    .split('<button')
    .slice(1)
    .map((s) => `<button${s.split('</button>')[0]}`)
    .filter((s) => s.includes('role="tab"'));
}

/** Each panel's opening tag, in order. */
function panels(out: string): string[] {
  return out
    .split('role="tabpanel"')
    .slice(1)
    .map((s) => `role="tabpanel"${s.slice(0, s.indexOf('>'))}`);
}

/** The attribute's value in a tag, or null. */
function attr(tag: string, name: string): string | null {
  return tag.match(new RegExp(`${name}="([^"]*)"`))?.[1] ?? null;
}

describe('SettingsTabs: roles and wiring', () => {
  it('a labelled tablist with one tab per entry, and one tabpanel per entry', () => {
    const out = html();
    expect(out).toContain('role="tablist"');
    expect(out).toContain('aria-label="AI settings sections"');
    expect(tabs(out)).toHaveLength(2);
    expect(panels(out)).toHaveLength(2);
    expect(tabs(out)[0]).toContain('>Models<');
    expect(tabs(out)[1]).toContain('>Approvals<');
  });

  it('each tab controls its panel, and each panel is labelled by its tab', () => {
    const out = html();
    tabs(out).forEach((tag, i) => {
      const controls = attr(tag, 'aria-controls');
      const tabId = attr(tag, 'id');
      expect(controls).not.toBeNull();
      expect(tabId).not.toBeNull();
      const panel = panels(out).find((p) => attr(p, 'id') === controls);
      expect(panel, `panel ${controls} for tab ${i}`).toBeDefined();
      expect(attr(panel as string, 'aria-labelledby')).toBe(tabId);
    });
  });

  it('the ids are unique, so two strips on one page do not clash', () => {
    const out = html();
    const ids = [...out.matchAll(/ id="([^"]*)"/g)].map((m) => m[1]);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe('SettingsTabs: selection and the roving tab stop', () => {
  it('the selected tab has aria-selected=true and tabindex 0; the others are false and -1', () => {
    const [models, approvals] = tabs(html({ selected: 'approvals' }));
    expect(attr(models, 'aria-selected')).toBe('false');
    expect(attr(models, 'tabindex')).toBe('-1');
    expect(attr(approvals, 'aria-selected')).toBe('true');
    expect(attr(approvals, 'tabindex')).toBe('0');
  });

  it('only the selected panel is shown; the others are hidden but still rendered', () => {
    const out = html({ selected: 'models' });
    const [models, approvals] = panels(out);
    expect(models).not.toContain('hidden');
    expect(approvals).toContain('hidden=""');
    expect(out).toContain('PANEL-models');
    expect(out).toContain('PANEL-approvals');
  });

  it('the active tab is marked for styling', () => {
    const [models, approvals] = tabs(html({ selected: 'models' }));
    expect(models).toContain('is-active');
    expect(approvals).not.toContain('is-active');
  });
});

describe('SettingsTabs: unsaved changes', () => {
  it('no dot and no hidden text when nothing is dirty', () => {
    const out = html();
    expect(out).not.toContain('tab-dot');
    expect(out).not.toContain('unsaved changes');
  });

  it('a dirty tab gets the dot and the "unsaved changes" text, and only that tab', () => {
    const [models, approvals] = tabs(html({ dirty: ['approvals'] }));
    expect(models).not.toContain('tab-dot');
    expect(models).not.toContain('unsaved changes');
    expect(approvals).toContain('<span class="tab-dot" aria-hidden="true"></span>');
    expect(approvals).toContain('unsaved changes');
    // A space before the text, so the accessible name reads "Approvals unsaved changes", not "Approvalsunsaved changes".
    expect(approvals).toContain('> unsaved changes</span>');
  });

  it('the dot is hidden from screen readers; the text is the accessible label', () => {
    const [, approvals] = tabs(html({ dirty: ['approvals'] }));
    expect(approvals).toContain('aria-hidden="true"');
    expect(approvals.indexOf('unsaved changes')).toBeGreaterThan(approvals.indexOf('>Approvals<'));
  });
});

describe('SettingsTabs: the panel content', () => {
  it('each panel holds what the caller gives for its id', () => {
    const out = html({ panel: (id) => (id === 'models' ? <i>MODELS-BODY</i> : <i>OTHER-BODY</i>) });
    const body = (id: string) => out.match(new RegExp(`<div[^>]*role="tabpanel"[^>]*id="[^"]*-panel-${id}"[^>]*>(.*?)</div>`))?.[1] ?? '';
    expect(body('models')).toContain('MODELS-BODY');
    expect(body('models')).not.toContain('OTHER-BODY');
    expect(body('approvals')).toContain('OTHER-BODY');
  });
});
