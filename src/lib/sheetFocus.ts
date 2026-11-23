/**
 * The index of the stop Tab (or Shift+Tab) moves to, among `count` focusable stops, from `current` (-1 when focus is not on
 * a stop). Wraps at both ends, so focus stays inside a sheet.
 */
export function nextStop(count: number, current: number, backwards: boolean): number {
  if (count <= 0) return -1;
  if (backwards) return current <= 0 ? count - 1 : current - 1;
  return current < 0 || current >= count - 1 ? 0 : current + 1;
}
