import { describe, expect, it } from 'vitest';
import { BUSY_NOTICE, BUSY_TIP, SIDE_BUSY_NOTICE, SWITCHING_NOTICE, switchRefusal } from './switchGuard';

const IDLE = { busy: false, sideBusy: false, switching: false };

describe('switchRefusal', () => {
  it('allows a switch when nothing runs', () => {
    expect(switchRefusal(IDLE)).toBeNull();
  });

  it('refuses while a turn runs, with the busy notice', () => {
    expect(switchRefusal({ ...IDLE, busy: true })).toBe(BUSY_NOTICE);
  });

  it('refuses while a side command runs, with the side-command notice', () => {
    expect(switchRefusal({ ...IDLE, sideBusy: true })).toBe(SIDE_BUSY_NOTICE);
  });

  it('refuses while another switch is in flight', () => {
    expect(switchRefusal({ ...IDLE, switching: true })).toBe(SWITCHING_NOTICE);
  });

  it('the side command reason comes first when both run', () => {
    expect(switchRefusal({ busy: true, sideBusy: true, switching: true })).toBe(SIDE_BUSY_NOTICE);
  });

  it('the tooltip for blocked controls names the same reason as the History rows', () => {
    expect(BUSY_TIP).toBe('Wait for the agent to finish, or cancel it with Ctrl-C, first');
  });
});
