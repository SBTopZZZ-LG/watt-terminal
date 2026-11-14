// The approval countdown: how long a pending approval card shows before it expires. Pure, so the
// wording is tested without the DOM. The transcript stamps each card with its expiry (see transcript.ts).

/** How long an approval card waits before it expires, in ms. Mirrors the backend's approval timeout. */
export const APPROVAL_WINDOW_MS = 120_000;

/** True once the countdown has run out: the card's expiry is now or past. */
export function approvalExpired(expiresAt: number, now: number): boolean {
  return now >= expiresAt;
}

/**
 * The countdown line for a pending approval card: `expires in m:ss` while time remains, `expired` once
 * it has run out.
 */
export function remainingLabel(expiresAt: number, now: number): string {
  const remaining = expiresAt - now;
  if (remaining <= 0) {
    return 'expired';
  }
  const secs = Math.floor(remaining / 1000);
  return `expires in ${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
}
