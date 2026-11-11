import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { StatusChip } from './StatusChip';
import type { LlmStatus, SettingsStateView } from '../lib/types';

const LLM_ON: LlmStatus = { enabled: true, model: 'big', provider: 'Acme' };
const LLM_OFF: LlmStatus = { enabled: false, model: null, provider: null };
const REASON = 'settings are invalid: not valid JSON';
const BROKEN: SettingsStateView = { kind: 'broken', reason: REASON, last_good_available: true, path: '/config/settings.json' };
const LAST_GOOD: SettingsStateView = { kind: 'last_good', reason: REASON, last_good_available: true, path: '/config/settings.json' };
const OK: SettingsStateView = { kind: 'ok', reason: null, last_good_available: false, path: '/config/settings.json' };

function chip(llm: LlmStatus, settingsState: SettingsStateView | null) {
  return renderToStaticMarkup(
    <StatusChip
      integration="live"
      observer="ok"
      input="prompt"
      cwd={null}
      autoApprove={false}
      onToggleAutoApprove={() => {}}
      llm={llm}
      settingsState={settingsState}
      onOpenSettings={() => {}}
    />,
  );
}

/** The model chip's opening tag and text, as markup, or ''. */
function modelChip(out: string): string {
  return out.match(/<[a-z]+[^>]*chip-model[^>]*>[^<]*<\/[a-z]+>/)?.[0] ?? '';
}

/** The settings button's opening tag, or ''. */
function gearTag(out: string): string {
  return out.match(/<button[^>]*aria-label="AI settings[^"]*"[^>]*>/)?.[0] ?? '';
}

describe('StatusChip: the model chip is a label, not a button', () => {
  it('ok: the model, in the normal colour, and it is not a button', () => {
    const out = chip(LLM_ON, OK);
    const label = modelChip(out);
    expect(label).toContain('model: big');
    expect(label).not.toContain('chip-warn');
    expect(label.startsWith('<span')).toBe(true);
    expect(label).not.toContain('type="button"');
    expect(out).not.toMatch(/<button[^>]*chip-model/);
  });

  it('no settings state at all (no app folders) behaves as ok', () => {
    expect(modelChip(chip(LLM_ON, null))).toContain('model: big');
    expect(modelChip(chip(LLM_OFF, null))).toContain('mock agent');
  });

  it('broken: "settings error" in the warning colour, with the reason in the hover text', () => {
    const label = modelChip(chip(LLM_OFF, BROKEN));
    expect(label).toContain('settings error');
    expect(label).toContain('chip-warn');
    expect(label).toContain(`title="settings.json is broken: ${REASON}"`);
    expect(label.startsWith('<span')).toBe(true);
  });

  it('last good: "last good" in the warning colour', () => {
    const label = modelChip(chip(LLM_ON, LAST_GOOD));
    expect(label).toContain('last good');
    expect(label).toContain('chip-warn');
  });

  it('ok: the hover text is the provider and the model', () => {
    expect(modelChip(chip(LLM_ON, OK))).toContain('title="Acme · model: big"');
  });
});

describe('StatusChip: the AI settings button', () => {
  it('is a button named "AI settings", with the shortcut in its title', () => {
    const tag = gearTag(chip(LLM_ON, OK));
    expect(tag).toContain('aria-label="AI settings"');
    expect(tag).toContain('title="AI settings (Ctrl/Cmd+,)"');
    expect(tag).toContain('type="button"');
    expect(tag).not.toContain('chip-warn');
  });

  it('ok: no attention dot', () => {
    expect(chip(LLM_ON, OK)).not.toContain('chip-dot');
    expect(chip(LLM_ON, null)).not.toContain('chip-dot');
  });

  it('broken: the warning colour, a dot, and the name says it needs attention', () => {
    const out = chip(LLM_OFF, BROKEN);
    const tag = gearTag(out);
    expect(tag).toContain('aria-label="AI settings (needs attention)"');
    expect(tag).toContain('chip-warn');
    expect(out).toContain('<span class="chip-dot" aria-hidden="true"></span>');
  });

  it('last good: the same attention state as broken', () => {
    const out = chip(LLM_ON, LAST_GOOD);
    expect(gearTag(out)).toContain('aria-label="AI settings (needs attention)"');
    expect(gearTag(out)).toContain('chip-warn');
    expect(out).toContain('chip-dot');
  });

  it('is the last control of the strip, after the auto-approve button', () => {
    const out = chip(LLM_ON, OK);
    const gearAt = out.indexOf('aria-label="AI settings"');
    expect(out.indexOf('auto-approve')).toBeLessThan(gearAt);
    // The gear's own <button opens at the last <button in the strip.
    expect(out.lastIndexOf('<button', gearAt)).toBe(out.lastIndexOf('<button'));
  });
});
