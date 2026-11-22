// Platform-dependent keys and text. The app was written for macOS, and every place that differs on Windows goes through
// here. Pure: the platform info is a parameter, so both modes are unit-tested.
//
// Without a window (vitest runs in node) the answer is macOS, the behaviour the existing tests describe. In the webview
// the real platform is read.

/** The parts of the navigator that name the platform. A browser Navigator satisfies this. */
export interface PlatformInfo {
  userAgentData?: { platform?: string } | null;
  platform?: string;
  userAgent?: string;
}

function browserPlatformInfo(): PlatformInfo | null {
  if (typeof window === 'undefined' || typeof navigator === 'undefined') {
    return null;
  }
  return navigator;
}

/**
 * True on macOS. The first platform string that is set decides: userAgentData, then navigator.platform, then the user
 * agent. With no platform info at all the answer is true, the behaviour the app was written for.
 */
export function isMacPlatform(info: PlatformInfo | null = browserPlatformInfo()): boolean {
  if (info === null) {
    return true;
  }
  const text = info.userAgentData?.platform || info.platform || info.userAgent || '';
  return text === '' || /mac/i.test(text);
}

/** Whether the app runs on macOS, read once when the module loads. */
export const IS_MAC: boolean = isMacPlatform();

/** Where API keys are kept, for the settings text. */
export function secretStoreName(mac: boolean = IS_MAC): string {
  return mac ? 'macOS Keychain' : 'Windows Credential Manager';
}

/** The same store, short form, for hints that say "stored in the ...". */
export function secretStoreShortName(mac: boolean = IS_MAC): string {
  return mac ? 'Keychain' : 'Credential Manager';
}

/**
 * The context menu's hints (copy, cut, paste, select-all, find, find-terminal, clear) name the key that does the same thing:
 * Cmd on macOS, Ctrl elsewhere. Copy in the terminal is Ctrl+Shift+C off macOS (Ctrl+C there is the interrupt).
 */
export type ShortcutName = 'agent' | 'settings' | 'copy' | 'copy-terminal' | 'cut' | 'paste' | 'select-all' | 'find' | 'find-terminal' | 'clear';

/** The shortcut as the UI shows it in tooltips and hints. The macOS text is what the app showed before the port. */
export function shortcutLabel(name: ShortcutName, mac: boolean = IS_MAC): string {
  switch (name) {
    case 'agent':
      return mac ? 'Ctrl/Cmd+Shift+A' : 'Ctrl+Shift+A';
    case 'settings':
      return mac ? 'Ctrl/Cmd+,' : 'Ctrl+,';
    case 'copy':
      return mac ? 'Cmd+C' : 'Ctrl+C';
    case 'copy-terminal':
      return mac ? 'Cmd+C' : 'Ctrl+Shift+C';
    case 'cut':
      return mac ? 'Cmd+X' : 'Ctrl+X';
    case 'paste':
      return mac ? 'Cmd+V' : 'Ctrl+V';
    case 'select-all':
      return mac ? 'Cmd+A' : 'Ctrl+A';
    case 'find':
      return mac ? 'Cmd+F' : 'Ctrl+F';
    case 'find-terminal':
      return mac ? 'Cmd+Shift+F' : 'Ctrl+Shift+F';
    case 'clear':
      return mac ? 'Cmd+K' : 'Ctrl+Shift+K';
  }
}

/** The font stack for the xterm grid. Cascadia Mono ships with Windows 11. */
export function terminalFontFamily(mac: boolean = IS_MAC): string {
  return mac
    ? 'Menlo, Monaco, "SF Mono", ui-monospace, monospace'
    : '"Cascadia Mono", Consolas, Menlo, Monaco, "SF Mono", ui-monospace, monospace';
}
