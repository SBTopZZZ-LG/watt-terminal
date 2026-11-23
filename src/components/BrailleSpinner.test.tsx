import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { BRAILLE_FRAMES, BRAILLE_STILL, brailleFrame, BrailleSpinner } from './BrailleSpinner';

describe('brailleFrame', () => {
  it('cycles through the frames and wraps around', () => {
    expect(BRAILLE_FRAMES).toHaveLength(10);
    expect(brailleFrame(0, false)).toBe('⠋');
    expect(brailleFrame(1, false)).toBe('⠙');
    expect(brailleFrame(9, false)).toBe('⠏');
    expect(brailleFrame(10, false)).toBe('⠋');
    expect(brailleFrame(-1, false)).toBe('⠏');
  });

  it('shows one still frame under reduced motion, whatever the tick', () => {
    expect(BRAILLE_STILL).toBe('⠶');
    for (const tick of [0, 1, 5, 42]) {
      expect(brailleFrame(tick, true)).toBe(BRAILLE_STILL);
    }
  });
});

describe('BrailleSpinner', () => {
  it('renders the first frame, hidden from assistive tech', () => {
    expect(renderToStaticMarkup(<BrailleSpinner />)).toBe('<span class="braille-spinner" aria-hidden="true">⠋</span>');
  });
});
