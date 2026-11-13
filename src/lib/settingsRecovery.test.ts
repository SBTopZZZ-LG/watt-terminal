import { describe, expect, it } from 'vitest';
import { bannerText, chipBrokenTitle, chipLabel, recoveryExplanation, recoveryKind, reasonText } from './settingsRecovery';
import type { SettingsStateView } from './types';

const PATH = '/config/settings.json';
const OK: SettingsStateView = { kind: 'ok', reason: null, last_good_available: false, path: PATH };
const BROKEN: SettingsStateView = { kind: 'broken', reason: 'settings are invalid: not valid JSON', last_good_available: true, path: PATH };
const LAST_GOOD: SettingsStateView = { kind: 'last_good', reason: 'settings are invalid: not valid JSON', last_good_available: true, path: PATH };
const LLM_ON = { enabled: true, model: 'big', provider: 'Acme' };
const LLM_OFF = { enabled: false, model: null, provider: null };

describe('recoveryKind', () => {
  it('a missing state is ok, and each state names its own kind', () => {
    expect(recoveryKind(null)).toBe('ok');
    expect(recoveryKind(undefined)).toBe('ok');
    expect(recoveryKind(OK)).toBe('ok');
    expect(recoveryKind(BROKEN)).toBe('broken');
    expect(recoveryKind(LAST_GOOD)).toBe('last_good');
  });
});

describe('bannerText', () => {
  it('is null while the settings are fine', () => {
    expect(bannerText(null)).toBeNull();
    expect(bannerText(OK)).toBeNull();
  });

  it('broken: the reason, and that the agent cannot send until it is fixed', () => {
    expect(bannerText(BROKEN)).toBe(
      'Settings file is broken: settings are invalid: not valid JSON. The agent cannot send messages until it is fixed.',
    );
  });

  it('last good: running on the last working settings, with the reason', () => {
    expect(bannerText(LAST_GOOD)).toBe('Running on the last working settings (settings.json is broken: settings are invalid: not valid JSON).');
  });

  it('the reason goes through the History sanitizer: bidi overrides and control characters do not reach the text', () => {
    const hostile = { ...BROKEN, reason: 'bad‮reason\u0007 here​' };
    const text = bannerText(hostile) ?? '';
    expect(text).toContain('badreason  here');
    expect(text).not.toMatch(/[‪-‮​\u0007]/);
  });

  it('a missing reason still reads as a sentence', () => {
    expect(bannerText({ ...BROKEN, reason: null })).toBe('Settings file is broken: the file could not be read. The agent cannot send messages until it is fixed.');
  });
});

describe('chipLabel', () => {
  it('shows the broken states in place of the model', () => {
    expect(chipLabel(BROKEN, LLM_OFF)).toBe('settings error');
    expect(chipLabel(LAST_GOOD, LLM_ON)).toBe('last good');
  });

  it('otherwise shows the model, or the mock agent, as before', () => {
    expect(chipLabel(null, LLM_ON)).toBe('model: big');
    expect(chipLabel(OK, { enabled: true, model: null })).toBe('model: unknown');
    expect(chipLabel(OK, LLM_OFF)).toBe('mock agent');
  });

  it('appends #variant when one is selected, and never on the broken or mock states', () => {
    expect(chipLabel(OK, { ...LLM_ON, variant: 'high' })).toBe('model: big#high');
    expect(chipLabel(OK, { ...LLM_ON, variant: null })).toBe('model: big');
    expect(chipLabel(OK, { ...LLM_OFF, variant: 'high' })).toBe('mock agent');
    expect(chipLabel(BROKEN, { ...LLM_ON, variant: 'high' })).toBe('settings error');
    expect(chipLabel(LAST_GOOD, { ...LLM_ON, variant: 'high' })).toBe('last good');
  });
});

describe('chipBrokenTitle and recoveryExplanation', () => {
  it('the title names the state and the reason, and is null while ok', () => {
    expect(chipBrokenTitle(OK)).toBeNull();
    expect(chipBrokenTitle(BROKEN)).toBe('settings.json is broken: settings are invalid: not valid JSON');
    expect(chipBrokenTitle(LAST_GOOD)).toBe('Running on the last working settings: settings.json is broken (settings are invalid: not valid JSON)');
  });

  it('the explanation differs by state', () => {
    expect(recoveryExplanation('broken')).toBe('Changes are disabled until the file is fixed, reverted or reloaded.');
    expect(recoveryExplanation('last_good')).toContain('stays as it is on disk');
  });
});

describe('reasonText', () => {
  it('trims and keeps ordinary text', () => {
    expect(reasonText({ ...BROKEN, reason: '  unexpected end  ' })).toBe('unexpected end');
  });
});
