import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { ApprovalCountdown } from './ApprovalCountdown';

describe('ApprovalCountdown', () => {
  it('renders nothing with no expiry', () => {
    expect(renderToStaticMarkup(<ApprovalCountdown />)).toBe('');
    expect(renderToStaticMarkup(<ApprovalCountdown expiresAt={undefined} />)).toBe('');
  });

  it('shows the remaining label when an expiry is set', () => {
    const html = renderToStaticMarkup(<ApprovalCountdown expiresAt={Date.now() + 120_000} />);
    expect(html).toContain('class="approval-countdown"');
    expect(html).toMatch(/expires in \d+:\d\d/);
  });

  it('says expired once the time is up', () => {
    const html = renderToStaticMarkup(<ApprovalCountdown expiresAt={Date.now() - 1_000} />);
    expect(html).toContain('class="approval-countdown"');
    expect(html).toContain('expired');
  });
});
