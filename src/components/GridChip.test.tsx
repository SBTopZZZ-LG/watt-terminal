import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { GridChip } from './GridChip';

describe('GridChip', () => {
  // The grid is read in an effect, so the server render has no info: the placeholder shows, with no tip.
  it('shows -- with no tip and no native title while the grid is unknown', () => {
    expect(renderToStaticMarkup(<GridChip />)).toBe('<span class="chip chip-grid">--</span>');
  });
});
