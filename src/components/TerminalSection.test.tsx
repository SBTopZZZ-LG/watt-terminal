import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { TerminalSection } from './TerminalSection';

const NOOP = () => undefined;

describe('TerminalSection', () => {
  it('shows the new-terminals setting with its helper text', () => {
    const html = renderToStaticMarkup(<TerminalSection hideMarks newTabsVisible={false} onHideMarksChange={NOOP} onNewTabsVisibleChange={NOOP} />);
    expect(html).toContain('New terminals are visible to the agent');
    expect(html).toContain('Off: a new tab is private to you until you assign it. On: new tabs join the current chat automatically.');
  });

  it('checks only the boxes that are on: hide marks on, new terminals off gives one checked box', () => {
    const html = renderToStaticMarkup(<TerminalSection hideMarks newTabsVisible={false} onHideMarksChange={NOOP} onNewTabsVisibleChange={NOOP} />);
    expect(html.match(/checked=""/g)).toHaveLength(1);
  });

  it('checks the new-terminals box when it is on', () => {
    const html = renderToStaticMarkup(<TerminalSection hideMarks={false} newTabsVisible onHideMarksChange={NOOP} onNewTabsVisibleChange={NOOP} />);
    expect(html.match(/checked=""/g)).toHaveLength(1);
  });
});
