import { describe, expect, it } from 'vitest';
import { AUTO_PER_TURN, askedBecause, decisionLabel, decisionTooltip, previewText, ranBecause } from './approvalReasons';
import type { ApprovalReasonCode } from './types';

const ASK_CODES: ApprovalReasonCode[] = [
  'manual_mode',
  'send_keys',
  'gate_chars',
  'gate_path',
  'gate_denylist',
  'deny_rule',
  'no_rule',
  'auto_cap',
  'nested_level',
  'approve_all_suspended',
];

describe('askedBecause', () => {
  it('gives every asking code its own sentence, starting "Asked because"', () => {
    const texts = ASK_CODES.map((code) => askedBecause(code));
    for (const text of texts) {
      expect(text).toMatch(/^Asked because .+\.$/);
    }
    expect(new Set(texts).size).toBe(ASK_CODES.length);
  });

  it('names the cap with its number', () => {
    expect(askedBecause('auto_cap')).toContain(`${AUTO_PER_TURN} automatic approvals`);
    expect(AUTO_PER_TURN).toBe(8);
  });

  it('says the character and path gates are not rules', () => {
    expect(askedBecause('gate_chars')).toContain('your rules do not apply');
    expect(askedBecause('gate_path')).toContain('your rules do not apply');
  });

  it('says approving send_keys latches typing until the user types, not that every keystroke needs approval', () => {
    expect(askedBecause('send_keys')).toBe(
      'Asked because approving lets the agent keep typing into that command until you type in the terminal.',
    );
  });

  it('is null without a reason', () => {
    expect(askedBecause(null)).toBeNull();
    expect(askedBecause(undefined)).toBeNull();
  });

  it('gives a generic line for a code this build does not know, never the raw code', () => {
    const text = askedBecause('future_code' as ApprovalReasonCode);
    expect(text).toBe('Asked for your approval.');
    expect(text).not.toContain('future_code');
  });

  it('does not read a prototype property as a reason', () => {
    expect(askedBecause('constructor' as ApprovalReasonCode)).toBe('Asked for your approval.');
  });
});

describe('ranBecause', () => {
  it('says why an automatic run happened, for both automatic codes', () => {
    expect(ranBecause('rule')).toBe('Ran without asking: an allow rule matches this command.');
    expect(ranBecause('approve_all')).toBe('Ran without asking: approve-all is on for this session.');
  });

  it('falls back to a generic line for an unknown code', () => {
    expect(ranBecause('future_code' as ApprovalReasonCode)).toBe('Ran without asking.');
  });
});

describe('decisionLabel', () => {
  it('shows the reason on an automatic run', () => {
    expect(decisionLabel('auto', 'rule')).toBe('auto · rule');
    expect(decisionLabel('auto', 'approve_all')).toBe('auto · approve_all');
  });

  it('shows the bare decision otherwise', () => {
    expect(decisionLabel('auto', null)).toBe('auto');
    expect(decisionLabel('manual', 'gate_chars')).toBe('manual');
    expect(decisionLabel('denied', 'no_rule')).toBe('denied');
    expect(decisionLabel('read', null)).toBe('read');
    expect(decisionLabel('refused', null)).toBe('refused');
  });
});

describe('decisionTooltip', () => {
  it('says why an automatic run happened', () => {
    expect(decisionTooltip('auto', 'rule')).toBe(ranBecause('rule'));
  });

  it('says why a manual or denied call asked', () => {
    expect(decisionTooltip('manual', 'gate_denylist')).toBe(askedBecause('gate_denylist'));
    expect(decisionTooltip('denied', 'no_rule')).toBe(askedBecause('no_rule'));
  });

  it('has none for the other decisions, or without a reason', () => {
    expect(decisionTooltip('read', null)).toBeNull();
    expect(decisionTooltip('refused', null)).toBeNull();
    expect(decisionTooltip('auto', null)).toBeNull();
    expect(decisionTooltip('manual', null)).toBeNull();
  });
});

describe('previewText', () => {
  it('reports an automatic verdict with its code, as the Try a command line', () => {
    expect(previewText({ verdict: 'auto', reason: 'rule' })).toBe('Would run without asking (rule)');
  });

  it('reports an ask with the clause for its reason', () => {
    expect(previewText({ verdict: 'ask', reason: 'no_rule' })).toBe('Would ask: no allow rule matches this command.');
    expect(previewText({ verdict: 'ask', reason: 'gate_chars' })).toContain('Would ask: the command has a character outside');
  });

  it('gives a generic ask for an unknown reason', () => {
    expect(previewText({ verdict: 'ask', reason: 'future_code' as ApprovalReasonCode })).toBe(
      'Would ask: the saved settings ask for this command.',
    );
  });

  it('uses the same clause as the card, after its "Asked because"', () => {
    for (const code of ASK_CODES) {
      expect(askedBecause(code)).toBe(`Asked because ${previewText({ verdict: 'ask', reason: code }).replace('Would ask: ', '')}`);
    }
  });

  it('an unintegrated shell is a sentence of its own, without "Asked because"', () => {
    expect(askedBecause('unintegrated_shell')).toBe('Typed into a shell without integration: your rules cannot check this.');
  });
});

describe('send_keys reasons that run without asking', () => {
  it('an interrupt and a latched keystroke say they ran without asking, with the reason in the tooltip', () => {
    expect(ranBecause('interrupt')).toBe('Ran without asking: Ctrl-C, Ctrl-D or Esc into the command the agent started.');
    expect(ranBecause('latched')).toBe(
      "Ran without asking: you approved this command's first keystrokes, so its later ones run too until you type.",
    );
    expect(decisionTooltip('auto', 'latched')).toBe(ranBecause('latched'));
    expect(decisionTooltip('auto', 'interrupt')).toBe(ranBecause('interrupt'));
  });

  it('the row label names them: auto · interrupt, auto · latched', () => {
    expect(decisionLabel('auto', 'interrupt')).toBe('auto · interrupt');
    expect(decisionLabel('auto', 'latched')).toBe('auto · latched');
  });

  it('an unintegrated shell asks, so its tooltip is the card sentence', () => {
    expect(decisionTooltip('manual', 'unintegrated_shell')).toBe('Typed into a shell without integration: your rules cannot check this.');
  });
});
