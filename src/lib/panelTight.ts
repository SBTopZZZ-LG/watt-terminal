// The tight flag of a fixed-size agent panel: set while the panel cannot hold its parts at their least sizes (the transcript's
// floor and the todo list's two rows). App.css reads it (data-tight) and gives way: the todo list collapses to its toggle, the
// body's floor goes, and the transcript shrinks before the bar and the meter do. The flag is written on the agent panel's element,
// not by React, and it is measured with the flag off, so a panel that gains room clears it.
// The scroll position of the element kept across that measure (the transcript body) is preserved.

/** The attribute on the agent panel. */
export const TIGHT_ATTR = 'data-tight';

/** How far the panel may overflow before it counts as short of room, in px: above sub-pixel rounding. */
const TIGHT_OVERFLOW_PX = 1.5;

/**
 * Sets or clears the flag from the panel's overflow. Only a fixed-size panel can be tight; a missing panel does nothing.
 * With `keep`, its scrollTop is restored after the measure: the measure can clamp it, and the browser clamps the restore to the real max.
 */
export function settleTight(agent: HTMLElement | null, keep?: HTMLElement | null): void {
  if (!agent) {
    return;
  }
  const saved = keep?.scrollTop;
  agent.removeAttribute(TIGHT_ATTR);
  setFlag(agent);
  if (keep && saved !== undefined && keep.scrollTop !== saved) {
    keep.scrollTop = saved;
  }
}

/** Sets the flag if a sized panel overflows, with the flag already removed. */
function setFlag(agent: HTMLElement): void {
  if (!agent.classList.contains('is-sized')) {
    return;
  }
  if (agent.scrollHeight - agent.clientHeight > TIGHT_OVERFLOW_PX) {
    agent.setAttribute(TIGHT_ATTR, '');
  }
}
