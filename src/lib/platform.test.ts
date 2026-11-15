import { describe, expect, it } from 'vitest';
import { IS_MAC, isMacPlatform, secretStoreName, secretStoreShortName, shortcutLabel, terminalFontFamily } from './platform';

describe('isMacPlatform', () => {
  it('reads macOS from navigator.platform, from the user agent, or from userAgentData', () => {
    expect(isMacPlatform({ platform: 'MacIntel' })).toBe(true);
    expect(isMacPlatform({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15' })).toBe(true);
    expect(isMacPlatform({ userAgentData: { platform: 'macOS' } })).toBe(true);
  });

  it('reads Windows, Linux and other systems as not macOS', () => {
    expect(isMacPlatform({ platform: 'Win32' })).toBe(false);
    expect(isMacPlatform({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/120' })).toBe(false);
    expect(isMacPlatform({ platform: 'Linux x86_64' })).toBe(false);
  });

  it('prefers userAgentData, then navigator.platform, then the user agent', () => {
    expect(isMacPlatform({ userAgentData: { platform: 'Windows' }, platform: 'MacIntel' })).toBe(false);
    expect(isMacPlatform({ userAgentData: { platform: '' }, platform: 'Win32', userAgent: 'Macintosh' })).toBe(false);
    expect(isMacPlatform({ platform: 'Win32', userAgent: 'Macintosh' })).toBe(false);
  });

  it('answers macOS when there is no platform information, the behaviour the app was written for', () => {
    expect(isMacPlatform(null)).toBe(true);
    expect(isMacPlatform({})).toBe(true);
  });

  it('IS_MAC is true under vitest, which has no window', () => {
    expect(IS_MAC).toBe(true);
  });
});

describe('secret store wording', () => {
  it('names the macOS Keychain on Mac and the Windows Credential Manager elsewhere', () => {
    expect(secretStoreName(true)).toBe('macOS Keychain');
    expect(secretStoreName(false)).toBe('Windows Credential Manager');
    expect(secretStoreShortName(true)).toBe('Keychain');
    expect(secretStoreShortName(false)).toBe('Credential Manager');
  });
});

describe('shortcutLabel', () => {
  it('keeps the macOS labels unchanged', () => {
    expect(shortcutLabel('agent', true)).toBe('Ctrl/Cmd+Shift+A');
    expect(shortcutLabel('settings', true)).toBe('Ctrl/Cmd+,');
  });

  it('shows the Windows labels on other platforms', () => {
    expect(shortcutLabel('agent', false)).toBe('Ctrl+Shift+A');
    expect(shortcutLabel('settings', false)).toBe('Ctrl+,');
  });
});

describe('terminalFontFamily', () => {
  it('keeps the macOS stack unchanged', () => {
    expect(terminalFontFamily(true)).toBe('Menlo, Monaco, "SF Mono", ui-monospace, monospace');
  });

  it('puts Cascadia Mono and Consolas first elsewhere', () => {
    expect(terminalFontFamily(false)).toBe('"Cascadia Mono", Consolas, Menlo, Monaco, "SF Mono", ui-monospace, monospace');
  });
});
