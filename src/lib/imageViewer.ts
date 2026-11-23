// The image viewer's size rule. Pure, so it is tested without the DOM. The image keeps its aspect ratio: a big image is scaled
// down to fit the box, a small one is scaled up (its long edge to at least MIN_LONG_EDGE, and no more than MAX_UPSCALE times
// its natural size), and one in between is shown at its natural size. Nothing is zoomed beyond that.

export const VIEWER = {
  /** The box an image may fill: this share of the viewport's width and height, which leaves room for the close button and caption. */
  widthShare: 0.9,
  heightShare: 0.8,
  /** A small image is scaled up until its long edge reaches this many px ... */
  minLongEdge: 320,
  /** ... but never beyond this multiple of its natural size, so a tiny icon does not become a blur. */
  maxUpscale: 4,
} as const;

export interface Size {
  w: number;
  h: number;
}

/**
 * The shown size of an image: `natural` is its pixel size, `viewport` the window's size (CSS px). A natural size that is not
 * a positive finite pair is replaced by a square at the minimum edge, so something sane is always shown.
 */
export function fitImage(natural: Size, viewport: Size): Size {
  const valid = Number.isFinite(natural.w) && Number.isFinite(natural.h) && natural.w > 0 && natural.h > 0;
  const base = valid ? natural : { w: VIEWER.minLongEdge, h: VIEWER.minLongEdge };
  const maxW = Math.max(1, viewport.w * VIEWER.widthShare);
  const maxH = Math.max(1, viewport.h * VIEWER.heightShare);
  // The largest scale at which the image still fits the box.
  const fit = Math.min(maxW / base.w, maxH / base.h);
  const long = Math.max(base.w, base.h);
  let scale: number;
  if (fit < 1) {
    // Too big: scale down to the box.
    scale = fit;
  } else if (long < VIEWER.minLongEdge) {
    // Too small: scale up, but not past the minimum edge's worth, the upscale cap, or the box.
    scale = Math.min(VIEWER.minLongEdge / long, VIEWER.maxUpscale, fit);
  } else {
    scale = 1;
  }
  return { w: Math.max(1, Math.round(base.w * scale)), h: Math.max(1, Math.round(base.h * scale)) };
}
