import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SettingsBanner } from './SettingsBanner';
import { Transcript } from './Transcript';
import { EMPTY_TRANSCRIPT } from '../lib/transcript';
import type { SettingsStateView } from '../lib/types';

const REASON = 'settings are invalid: not valid JSON';
const BROKEN: SettingsStateView = { kind: 'broken', reason: REASON, last_good_available: false, path: '/config/settings.json' };
const LAST_GOOD: SettingsStateView = { kind: 'last_good', reason: REASON, last_good_available: true, path: '/config/settings.json' };
const OK: SettingsStateView = { kind: 'ok', reason: null, last_good_available: false, path: '/config/settings.json' };
const NOOP = () => undefined;

describe('SettingsBanner', () => {
  it('broken: the reason, what it means for the agent, and the two buttons', () => {
    const out = renderToStaticMarkup(<SettingsBanner state={BROKEN} onOpenSettings={NOOP} onDismiss={NOOP} />);
    expect(out).toContain(`Settings file is broken: ${REASON}. The agent cannot send messages until it is fixed.`);
    expect(out).toContain('role="status"');
    expect(out).toContain('>Open settings</button>');
    expect(out).toContain('>Dismiss</button>');
  });

  it('last good: says the agent runs on the last working settings', () => {
    const out = renderToStaticMarkup(<SettingsBanner state={LAST_GOOD} onOpenSettings={NOOP} onDismiss={NOOP} />);
    expect(out).toContain(`Running on the last working settings (settings.json is broken: ${REASON}).`);
  });

  it('ok: nothing is rendered', () => {
    expect(renderToStaticMarkup(<SettingsBanner state={OK} onOpenSettings={NOOP} onDismiss={NOOP} />)).toBe('');
  });

  it('a hostile reason is sanitised, not rendered as markup', () => {
    const out = renderToStaticMarkup(
      <SettingsBanner state={{ ...BROKEN, reason: '<img src=x>‮' }} onOpenSettings={NOOP} onDismiss={NOOP} />,
    );
    expect(out).toContain('&lt;img src=x&gt;');
    expect(out).not.toContain('‮');
  });
});

describe('Transcript: the banner sits above the chat body, in the chat view only', () => {
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
        banner={banner}
      />,
    );

  it('shows above the body', () => {
    const out = render();
    expect(out).toContain('BANNER');
    expect(out.indexOf('BANNER')).toBeLessThan(out.indexOf('transcript-body'));
  });

  it('is not shown while the panel is closed or the History view is shown', () => {
    expect(render({ open: false })).not.toContain('BANNER');
    expect(render({ view: 'history' })).not.toContain('BANNER');
  });
});
