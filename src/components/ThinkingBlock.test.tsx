import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { TranscriptItem } from '../lib/transcript';
import type { ThinkingMode } from '../lib/types';
import { ThinkingBlock, ThinkingModeContext } from './ThinkingBlock';

type ThinkingItem = Extract<TranscriptItem, { kind: 'thinking' }>;

const streaming: ThinkingItem = { id: 1, kind: 'thinking', text: 'Weighing <b>options</b>', streaming: true, startedAt: 0 };
const ended: ThinkingItem = { id: 1, kind: 'thinking', text: 'Done.', streaming: false, startedAt: 1000, endedAt: 5000 };

function html(item: ThinkingItem, mode: ThinkingMode): string {
  return renderToStaticMarkup(
    <ThinkingModeContext.Provider value={mode}>
      <ThinkingBlock item={item} />
    </ThinkingModeContext.Provider>,
  );
}

describe('ThinkingBlock', () => {
  it('auto while streaming: open, with the spinner and Thinking', () => {
    const out = html(streaming, 'auto');
    expect(out).toContain('aria-expanded="true"');
    expect(out).toContain('braille-spinner');
    expect(out).toContain('Thinking…');
    expect(out).not.toContain('inert');
  });

  it('text is escaped plain text, not markup', () => {
    const out = html(streaming, 'auto');
    expect(out).toContain('Weighing &lt;b&gt;options&lt;/b&gt;');
    expect(out).not.toContain('<b>');
  });

  it('collapsed while streaming: closed and inert, but the spinner still shows', () => {
    const out = html(streaming, 'collapsed');
    expect(out).toContain('aria-expanded="false"');
    expect(out).toContain('inert=""');
    expect(out).toContain('braille-spinner');
  });

  it('after the reasoning ends: the duration label and no spinner', () => {
    const out = html(ended, 'auto');
    expect(out).toContain('Thought for 4s');
    expect(out).toContain('aria-expanded="false"');
    expect(out).not.toContain('braille-spinner');
  });

  it('expanded stays open after the end', () => {
    const out = html(ended, 'expanded');
    expect(out).toContain('aria-expanded="true"');
    expect(out).not.toContain('inert');
    expect(out).toContain('Done.');
  });
});
