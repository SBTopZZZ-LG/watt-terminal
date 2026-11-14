import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { StatusChip } from './StatusChip';
import type { Foreground, InputState, LevelInfo, LlmStatus, SettingsStateView } from '../lib/types';

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

  it('ok with a variant: the label and the hover text carry #variant', () => {
    const out = chip({ ...LLM_ON, variant: 'high' }, OK);
    expect(modelChip(out)).toContain('model: big#high');
    expect(modelChip(out)).toContain('title="Acme · model: big#high"');
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

  it("the gear is the strip's last control", () => {
    const out = chip(LLM_ON, OK);
    const gearAt = out.indexOf('aria-label="AI settings"');
    // The gear's own <button opens before its label, at the last <button in the strip.
    expect(out.lastIndexOf('<button', gearAt)).toBe(out.lastIndexOf('<button'));
  });
});

function strip(input: InputState, foreground: Foreground | null | undefined) {
  return renderToStaticMarkup(
    <StatusChip
      integration="live"
      observer="ok"
      input={input}
      foreground={foreground}
      cwd={null}
      llm={LLM_ON}
      settingsState={OK}
      onOpenSettings={() => {}}
    />,
  );
}

/** The state chip's opening tag and text, as markup, or ''. It is the chip that holds the input state's word. */
function stateChip(out: string): string {
  return out.match(/<span class="chip[^"]*"[^>]*>(?:RUNNING|running|prompt|full-screen app)[^<]*<\/span>/)?.[0] ?? '';
}

describe('StatusChip: the state chip names a program without shell integration', () => {
  const SSH: Foreground = { kind: 'ssh', command: 'ssh ubuntu@lidia' };

  it('running an ssh: RUNNING · the command, in the neutral foreground style, with the title', () => {
    const tag = stateChip(strip('running', SSH));
    expect(tag).toContain('chip-foreground');
    expect(tag).toContain('RUNNING · ssh ubuntu@lidia');
    expect(tag).toContain(
      'title="ssh ubuntu@lidia: a shell on another machine. It has no shell integration, so the agent can read the screen but cannot run commands here."',
    );
    expect(tag).not.toContain('chip-warn');
  });

  it('a long command is cut in the label', () => {
    const tag = stateChip(strip('running', { kind: 'container_exec', command: 'docker exec -it some-container-name /bin/bash -l' }));
    expect(tag).toContain('RUNNING · docker exec -it some-contai…');
  });

  it('an ordinary running command keeps the plain running chip, with no title', () => {
    const tag = stateChip(strip('running', { kind: 'other', command: 'sleep 2' }));
    expect(tag).toBe('<span class="chip">running</span>');
  });

  it('no hint keeps the plain running chip too', () => {
    expect(stateChip(strip('running', null))).toBe('<span class="chip">running</span>');
    expect(stateChip(strip('running', undefined))).toBe('<span class="chip">running</span>');
  });

  it('at the prompt, the chip reads prompt even if a stale hint is passed', () => {
    expect(stateChip(strip('prompt', SSH))).toBe('<span class="chip">prompt</span>');
  });

  it('the hint does not change the strip around the state chip', () => {
    const plain = strip('running', null);
    const named = strip('running', SSH);
    expect(named.replace(/<span class="chip chip-foreground"[^>]*>[^<]*<\/span>/, '<span class="chip">running</span>')).toBe(plain);
  });
});

/** The strip with the nested-shell props: the location and the Integrate offer. */
function nestedStrip(over: { location?: LevelInfo | null; canIntegrate?: boolean }) {
  return renderToStaticMarkup(
    <StatusChip
      integration="live"
      observer="ok"
      input="prompt"
      cwd={null}
      location={over.location ?? null}
      canIntegrate={over.canIntegrate ?? false}
      onIntegrate={() => {}}
      llm={LLM_ON}
      settingsState={OK}
      onOpenSettings={() => {}}
    />,
  );
}

const REMOTE: LevelInfo = { key: 1, kind: 'remote', label: 'ssh ubuntu@lidia', sh: 'zsh', host: 'lidia', user: 'ubuntu', state: 'prompt' };

describe('StatusChip: the location chip and the Integrate button', () => {
  it('no nested level: no location chip', () => {
    expect(nestedStrip({})).not.toContain('chip-location');
  });

  it('a nested level on top: its label in the location chip, with the full label in the title', () => {
    const out = nestedStrip({ location: REMOTE });
    expect(out).toMatch(/<span class="chip chip-location" title="ssh ubuntu@lidia: zsh as ubuntu@lidia\. [^"]*">ssh ubuntu@lidia<\/span>/);
  });

  it('Integrate is a labelled button only when it is offered', () => {
    expect(nestedStrip({ canIntegrate: false })).not.toContain('Integrate');
    const out = nestedStrip({ canIntegrate: true });
    expect(out).toMatch(/<button[^>]*chip-integrate[^>]*aria-label="Integrate this shell"[^>]*>Integrate<\/button>/);
  });

  it('Integrate is in the right-hand group: just before the gear', () => {
    const out = nestedStrip({ canIntegrate: true });
    const integrateAt = out.indexOf('chip-integrate');
    expect(integrateAt).toBeLessThan(out.indexOf('aria-label="AI settings"'));
    // Nothing but the gear's own button comes between Integrate and the gear.
    expect(out.slice(integrateAt, out.indexOf('aria-label="AI settings"'))).not.toContain('chip-location');
  });

  it('with no Integrate offer, the location chip keeps the full label in its title', () => {
    const out = nestedStrip({ location: REMOTE, canIntegrate: false });
    expect(out).not.toContain('Integrate');
    expect(out).toMatch(/<span class="chip chip-location" title="ssh ubuntu@lidia: zsh as ubuntu@lidia\. [^"]*">ssh ubuntu@lidia<\/span>/);
  });
});

describe('StatusChip: the agent integrates quietly (auto mode)', () => {
  function strip(over: { integrating?: boolean; canIntegrate?: boolean } = {}) {
    return renderToStaticMarkup(
      <StatusChip
        integration="live"
        observer="ok"
        input="running"
      cwd={null}
      llm={LLM_ON}
        settingsState={null}
        onOpenSettings={() => {}}
        integrating={over.integrating}
        canIntegrate={over.canIntegrate}
        onIntegrate={() => {}}
      />,
    );
  }

  it('shows the Integrating… chip while the agent integrates', () => {
    expect(strip({ integrating: true })).toContain('chip-integrating');
    expect(strip({ integrating: true })).toContain('Integrating…');
  });

  it('shows no chip otherwise', () => {
    expect(strip({ integrating: false })).not.toContain('chip-integrating');
    expect(strip({})).not.toContain('Integrating…');
  });

  it('is a label, not a button', () => {
    const out = strip({ integrating: true });
    expect(out).toMatch(/<span class="chip chip-integrating"[^>]*>Integrating…<\/span>/);
  });
});
