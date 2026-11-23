import { describe, expect, it } from 'vitest';
import { fitImage, VIEWER } from './imageViewer';

// A 1000 x 800 window: the box is 900 x 640 (90% of the width, 80% of the height).
const WINDOW = { w: 1000, h: 800 };

describe('fitImage', () => {
  it('scales a big image down to fit the box, keeping its aspect ratio', () => {
    expect(fitImage({ w: 3000, h: 2000 }, WINDOW)).toEqual({ w: 900, h: 600 });
    expect(fitImage({ w: 2000, h: 3000 }, WINDOW)).toEqual({ w: 427, h: 640 });
  });

  it('an image that is exactly the box is shown at that size', () => {
    expect(fitImage({ w: 900, h: 640 }, WINDOW)).toEqual({ w: 900, h: 640 });
  });

  it('an image in between the bounds is shown at its natural size', () => {
    expect(fitImage({ w: 500, h: 400 }, WINDOW)).toEqual({ w: 500, h: 400 });
  });

  it('scales a small image up until its long edge is the minimum', () => {
    expect(fitImage({ w: 200, h: 100 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: 160 });
    expect(fitImage({ w: 100, h: 200 }, WINDOW)).toEqual({ w: 160, h: VIEWER.minLongEdge });
  });

  it('a tiny image is scaled up at most MAX_UPSCALE times, so it stays small', () => {
    expect(VIEWER.maxUpscale).toBe(4);
    expect(fitImage({ w: 16, h: 16 }, WINDOW)).toEqual({ w: 64, h: 64 });
    expect(fitImage({ w: 10, h: 4 }, WINDOW)).toEqual({ w: 40, h: 16 });
  });

  it('extreme aspect ratios: the long edge is bounded by the box, the short edge follows', () => {
    expect(fitImage({ w: 4000, h: 10 }, WINDOW)).toEqual({ w: 900, h: 2 });
    expect(fitImage({ w: 10, h: 4000 }, WINDOW)).toEqual({ w: 2, h: 640 });
  });

  it('a natural size that is zero, negative or not finite falls back to a sane square', () => {
    expect(fitImage({ w: 0, h: 0 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: VIEWER.minLongEdge });
    expect(fitImage({ w: 0, h: 500 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: VIEWER.minLongEdge });
    expect(fitImage({ w: -3, h: 10 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: VIEWER.minLongEdge });
    expect(fitImage({ w: Number.NaN, h: 10 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: VIEWER.minLongEdge });
    expect(fitImage({ w: Number.POSITIVE_INFINITY, h: 10 }, WINDOW)).toEqual({ w: VIEWER.minLongEdge, h: VIEWER.minLongEdge });
  });

  it('a window smaller than the minimum edge: the box wins over the minimum', () => {
    // The box is 270 x 160 here, so a 200 x 100 image grows only to the box's width.
    expect(fitImage({ w: 200, h: 100 }, { w: 300, h: 200 })).toEqual({ w: 270, h: 135 });
    // Every result is inside the box.
    const small = fitImage({ w: 16, h: 16 }, { w: 300, h: 200 });
    expect(small.w).toBeLessThanOrEqual(270);
    expect(small.h).toBeLessThanOrEqual(160);
  });

  it('a window of no size still gives at least one pixel each way', () => {
    const out = fitImage({ w: 500, h: 400 }, { w: 0, h: 0 });
    expect(out.w).toBeGreaterThanOrEqual(1);
    expect(out.h).toBeGreaterThanOrEqual(1);
  });
});
