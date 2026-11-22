import { describe, expect, it } from 'vitest';
import { gridTip } from './gridTip';

describe('gridTip', () => {
  it('lists the xterm grid, the host and grid widths, and the device pixel ratio, one per line', () => {
    expect(gridTip({ cols: 120, rows: 30, hostWidth: 900, screenWidth: 912, devicePixelRatio: 2 })).toBe(
      'xterm cols 120, rows 30\nhost 900px, grid 912px\ndevicePixelRatio 2',
    );
  });
});
