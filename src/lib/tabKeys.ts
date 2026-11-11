// Keyboard moves for a tab strip, as the WAI-ARIA tabs pattern gives them. Pure, so the wrap-around and the Home and End rules
// are tested without the DOM. The component does the focusing and the selecting.

/**
 * The id of the tab a key moves to, or null when the key is not a tab key or the current tab is unknown. ArrowRight and
 * ArrowLeft wrap from the last tab to the first and back. Home and End go to the first and the last.
 */
export function nextTabId(ids: readonly string[], current: string, key: string): string | null {
  const n = ids.length;
  const i = ids.indexOf(current);
  if (n === 0 || i === -1) {
    return null;
  }
  switch (key) {
    case 'ArrowRight':
      return ids[(i + 1) % n];
    case 'ArrowLeft':
      return ids[(i - 1 + n) % n];
    case 'Home':
      return ids[0];
    case 'End':
      return ids[n - 1];
    default:
      return null;
  }
}
