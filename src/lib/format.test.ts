import { describe, expect, it } from 'vitest';
import { formatCwd, previewJson, shortenPath } from './format';

describe('shortenPath', () => {
  it('replaces a macOS or Linux home with ~', () => {
    expect(shortenPath('/Users/sbtopzzz')).toBe('~');
    expect(shortenPath('/Users/sbtopzzz/Developer/agent-term')).toBe('~/Developer/agent-term');
    expect(shortenPath('/home/me/src')).toBe('~/src');
  });

  it('leaves other paths alone, including ones that only start like a home', () => {
    expect(shortenPath('/usr/local/bin')).toBe('/usr/local/bin');
    expect(shortenPath('/Users')).toBe('/Users');
    expect(shortenPath('/Usersx/a')).toBe('/Usersx/a');
  });
});

describe('formatCwd', () => {
  it('shows unknown until the shell reports a directory', () => {
    expect(formatCwd(null)).toBe('unknown');
  });

  it('shows only the shortened path, never the host', () => {
    expect(formatCwd({ host: 'mac', path: '/Users/sbtopzzz' })).toBe('~');
    expect(formatCwd({ host: 'mac', path: '/Users/sbtopzzz/Developer' })).toBe('~/Developer');
    expect(formatCwd({ host: 'mac', path: '/tmp' })).toBe('/tmp');
    expect(formatCwd({ host: 'mac', path: '/tmp' })).not.toContain('mac');
  });
});

describe('previewJson', () => {
  it('pretty-prints small values', () => {
    expect(previewJson({ ok: true })).toBe('{\n  "ok": true\n}');
  });

  it('cuts large values and says so', () => {
    const text = previewJson('x'.repeat(100), 20);
    expect(text.startsWith('"xxxxxxxxxxxxxxxxxxx')).toBe(true);
    expect(text.endsWith('... (cut)')).toBe(true);
  });
});
