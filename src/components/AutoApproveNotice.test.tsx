import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { AutoApproveNotice } from './AutoApproveNotice';
import { Transcript } from './Transcript';
import { EMPTY_TRANSCRIPT } from '../lib/transcript';

const NOOP = () => undefined;
const TEXT = 'Auto-approve is ON: the agent runs every command without asking. It resets when the app restarts.';

describe('AutoApproveNotice', () => {
  it('says what is on, and that it resets on restart', () => {
    const html = renderToStaticMarkup(<AutoApproveNotice onTurnOff={NOOP} />);
    expect(html).toContain(TEXT);
    expect(html).toContain('role="status"');
  });

  it('has a Turn off button and no dismiss: it stays until it is turned off', () => {
    const html = renderToStaticMarkup(<AutoApproveNotice onTurnOff={NOOP} />);
    expect(html).toContain('>Turn off</button>');
    expect(html).not.toContain('Dismiss');
  });
});

describe('Transcript: the auto-approve notice sits above the banner, in the chat view only', () => {
  const warning = <div data-part="warning">WARNING</div>;
  const banner = <div data-part="banner">BANNER</div>;
  const render = (over: { view?: 'chat' | 'history'; open?: boolean } = {}) =>
    renderToStaticMarkup(
      <Transcript
        items={EMPTY_TRANSCRIPT.items}
        busy={false}
        thinking={false}
        open={over.open ?? true}
        view={over.view ?? 'chat'}
        onToggle={NOOP}
        onApprove={NOOP}
        onNewConversation={NOOP}
        warning={warning}
        banner={banner}
      />,
    );

  it('shows above the banner and the body', () => {
    const out = render();
    expect(out).toContain('WARNING');
    expect(out.indexOf('WARNING')).toBeLessThan(out.indexOf('BANNER'));
    expect(out.indexOf('WARNING')).toBeLessThan(out.indexOf('transcript-body'));
  });

  it('is not shown while the panel is closed or the History view is shown', () => {
    expect(render({ open: false })).not.toContain('WARNING');
    expect(render({ view: 'history' })).not.toContain('WARNING');
  });
});
