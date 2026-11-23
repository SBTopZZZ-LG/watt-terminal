import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ThinkingMode } from '../lib/types';
import { ThinkingSection } from './ThinkingSection';

function html(thinkingMode: ThinkingMode): string {
  return renderToStaticMarkup(<ThinkingSection thinkingMode={thinkingMode} onThinkingModeChange={() => {}} />);
}

describe('ThinkingSection', () => {
  it('shows the Reasoning title and the three options with their labels', () => {
    const out = html('auto');
    expect(out).toContain('Reasoning</h3>');
    expect(out).toContain('Collapsed');
    expect(out).toContain('Auto');
    expect(out).toContain('Expanded');
    expect(out).toContain('Always show the full reasoning.');
  });

  it('checks the saved mode only', () => {
    const radio = (out: string, mode: ThinkingMode) => out.match(new RegExp(`<input[^>]*value="${mode}"[^>]*>`))?.[0] ?? '';
    const out = html('collapsed');
    expect(radio(out, 'collapsed')).toContain('checked=""');
    expect(radio(out, 'auto')).not.toContain('checked');
    expect(radio(html('expanded'), 'expanded')).toContain('checked=""');
  });
});
