import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { InputBar } from './InputBar';
import type { BarMode } from '../lib/inputState';

const NOOP = () => undefined;

const BASE = {
  mode: 'chat' as BarMode,
  hint: null,
  hintInline: false,
  hasPendingApproval: false,
  busy: false,
  locked: false,
  focusToken: 0,
  onFocusChange: NOOP,
  onRestart: NOOP,
  restarting: false,
  onAgent: async () => null,
  onInterrupt: NOOP,
  onApproveFirst: NOOP,
  onDenyFirst: NOOP,
  onClose: NOOP,
};

type Props = Partial<typeof BASE>;

const render = (over: Props = {}) => renderToStaticMarkup(<InputBar {...BASE} {...over} />);

/** The opening tag of the element that holds `needle`: searches back from the needle to the nearest `<tag`. */
function tagHolding(html: string, tag: string, needle: string): string {
  const at = html.indexOf(needle);
  expect(at, `${needle} is in the markup`).toBeGreaterThan(-1);
  const start = html.lastIndexOf(`<${tag}`, at);
  const end = html.indexOf('>', start);
  return html.slice(start, end + 1);
}

describe('InputBar: the agent label', () => {
  it('has no "agent" label beside the input', () => {
    const html = render();
    expect(html).not.toContain('mode-agent');
    expect(html).not.toContain('bar-mode');
    expect(html).not.toMatch(/>agent</);
  });
});

describe('InputBar: the input', () => {
  it('is a textarea with one row, and keeps the bar-input class', () => {
    const html = render();
    const tag = tagHolding(html, 'textarea', 'bar-input');
    expect(tag).toContain('rows="1"');
    expect(html).not.toContain('<input');
  });

  it('is disabled while locked', () => {
    expect(tagHolding(render({ locked: true }), 'textarea', 'bar-input')).toContain('disabled=""');
    expect(tagHolding(render(), 'textarea', 'bar-input')).not.toContain('disabled=""');
  });
});

describe('InputBar: the send button', () => {
  it('is Send, and disabled when the input is empty', () => {
    const tag = tagHolding(render(), 'button', 'bar-send');
    expect(tag).toContain('aria-label="Send"');
    expect(tag).toContain('disabled=""');
  });

  it('is disabled while locked, even when busy', () => {
    expect(tagHolding(render({ locked: true }), 'button', 'bar-send')).toContain('disabled=""');
    expect(tagHolding(render({ locked: true, busy: true }), 'button', 'bar-send')).toContain('disabled=""');
  });

  it('is Stop, enabled, and styled as a stop while the agent is busy', () => {
    const tag = tagHolding(render({ busy: true }), 'button', 'bar-send');
    expect(tag).toContain('aria-label="Stop"');
    expect(tag).toContain('is-stop');
    expect(tag).not.toContain('disabled=""');
  });

  it('is not Stop when idle', () => {
    const tag = tagHolding(render(), 'button', 'bar-send');
    expect(tag).not.toContain('is-stop');
  });

  it('is a plain button, so it does not submit a form', () => {
    expect(tagHolding(render(), 'button', 'bar-send')).toContain('type="button"');
  });
});

describe('InputBar: the busy light', () => {
  it('the field has the animated class only while busy', () => {
    expect(render({ busy: true })).toContain('class="bar-field is-busy"');
    expect(render({ busy: false })).not.toContain('is-busy');
  });
});

describe('InputBar: the shell modes have no input', () => {
  it('the exited mode has no textarea and no send button', () => {
    const html = render({ mode: 'exited' });
    expect(html).not.toContain('<textarea');
    expect(html).not.toContain('bar-send');
  });

  it('the collapsed mode has no textarea and no send button', () => {
    const html = render({ mode: 'collapsed' });
    expect(html).not.toContain('<textarea');
    expect(html).not.toContain('bar-send');
  });
});
