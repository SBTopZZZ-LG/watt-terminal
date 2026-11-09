// Paints the command stripes over the terminal. The stripes are plain elements in xterm's .xterm-screen, with the
// same origin as the grid, so row N sits at N times the cell height. Positions come from stripeSegments(), which cuts
// each stripe to the viewport. xterm's decorations are not used: they are hidden whenever their first row is out of
// view, which hid any command taller than the screen.
//
// The layer is hidden in the alternate screen (full-screen programs), where the shell's marks do not apply.
//
// Known limitation: after a window resize that reflows the buffer, a mark that sat on a wrapped row (not the first
// row of its logical line) can land on a neighbouring row. Accepted; the stripes are positional only.

import type { Terminal } from '@xterm/xterm';
import type { CommandMarks } from './commandMarks';
import { stripeSegments } from './stripes';

export class StripeLayer {
  private readonly layer: HTMLDivElement;
  private readonly screen: HTMLElement;
  private readonly pool: HTMLDivElement[] = [];
  private frame: number | undefined;

  constructor(
    private readonly term: Terminal,
    private readonly marks: CommandMarks,
  ) {
    const screen = term.element?.querySelector<HTMLElement>('.xterm-screen');
    if (!screen) {
      throw new Error('StripeLayer needs the terminal to be opened first');
    }
    this.screen = screen;
    this.layer = document.createElement('div');
    this.layer.className = 'stripe-layer';
    screen.appendChild(this.layer);

    term.onScroll(() => this.schedule());
    term.onWriteParsed(() => this.schedule());
    term.onResize(() => this.schedule());
    term.onRender(() => this.schedule());
    term.buffer.onBufferChange(() => this.schedule());
    this.schedule();
  }

  /** Repaints on the next animation frame. Several calls in one frame paint once. */
  schedule(): void {
    if (this.frame !== undefined) {
      return;
    }
    this.frame = requestAnimationFrame(() => {
      this.frame = undefined;
      this.paint();
    });
  }

  private paint(): void {
    const buf = this.term.buffer.active;
    this.layer.hidden = buf.type === 'alternate';
    if (this.layer.hidden) {
      return;
    }
    const rows = this.term.rows;
    const cellHeight = this.screen.getBoundingClientRect().height / rows;
    const segments = stripeSegments(this.marks.stripes(), buf.viewportY, rows);
    segments.forEach((seg, i) => {
      const el = this.elementAt(i);
      el.hidden = false;
      el.style.top = `${seg.firstRow * cellHeight}px`;
      el.style.height = `${seg.rowCount * cellHeight}px`;
      el.dataset.exit = seg.exit;
      el.dataset.agent = String(seg.agent);
      el.dataset.startVisible = String(seg.startVisible);
    });
    for (let i = segments.length; i < this.pool.length; i++) {
      this.pool[i].hidden = true;
    }
  }

  /** The i-th stripe element, created on first use and kept for reuse. */
  private elementAt(i: number): HTMLDivElement {
    let el = this.pool[i];
    if (!el) {
      el = document.createElement('div');
      el.className = 'stripe';
      this.layer.appendChild(el);
      this.pool.push(el);
    }
    return el;
  }
}
