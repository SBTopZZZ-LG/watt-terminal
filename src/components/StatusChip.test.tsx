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

/** The model chip's button, as markup. */
function modelButton(out: string): string {
  return out.match(/<button[^>]*chip-model[^>]*>[^<]*<\/button>/)?.[0] ?? '';
}

describe('StatusChip: the model chip and settings state', () => {
  it('ok: the model, in the normal colour', () => {
    const button = modelButton(chip(LLM_ON, OK));
    expect(button).toContain('model: big');
    expect(button).not.toContain('chip-warn');
  });

  it('no settings state at all (no app folders) behaves as ok', () => {
    expect(modelButton(chip(LLM_ON, null))).toContain('model: big');
    expect(modelButton(chip(LLM_OFF, null))).toContain('mock agent');
  });

  it('broken: "settings error" in the warning colour, with the reason in the hover text, and it still opens the dialog', () => {
    const out = chip(LLM_OFF, BROKEN);
    const button = modelButton(out);
    expect(button).toContain('settings error');
    expect(button).toContain('chip-warn');
    expect(button).toContain(`settings.json is broken: ${REASON}. Open AI settings`);
    expect(button).toContain('type="button"');
  });

  it('last good: "last good" in the warning colour', () => {
    const button = modelButton(chip(LLM_ON, LAST_GOOD));
    expect(button).toContain('last good');
    expect(button).toContain('chip-warn');
  });
});
