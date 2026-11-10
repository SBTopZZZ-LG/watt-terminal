// Paints the command stripes over the terminal. The stripes are plain elements in xterm's .xterm-screen, with the
// same origin as the grid, so row N sits at N times the cell height. Positions come from stripeSegments(), which cuts
// each stripe to the viewport. xterm's decorations are not used: they are hidden whenever their first row is out of
// view, which hid any command taller than the screen.
//
// The layer is hidden in the alternate screen (full-screen programs), where the shell's marks do not apply.
//
// Separator lines are 1px lines between a command's output and the next prompt. They are centred in the blank row
// the shell leaves before that prompt (see integration.zsh), or on the prompt row's top edge when that row is not blank.
//
// The agent badge sits on the separator line when the command has one in a blank row, right-aligned and centred on
// the line. Otherwise it sits on the stripe's first row, as before. The stripe never draws it while its separator row
// is blank, even when that row is out of view, so the badge does not jump while scrolling.
//
// Known limitation: after a window resize that reflows the buffer, a mark (stripe or separator) that sat on a wrapped
// row (not the first row of its logical line) can land on a neighbouring row. Accepted; the layer is positional only.

import type { Terminal } from '@xterm/xterm';
import type { CommandMarks } from './commandMarks';
import { badgeOnSeparator, separatorSegments, separatorTop, stripeSegments } from './stripes';

export class StripeLayer {
  private readonly layer: HTMLDivElement;
  private readonly screen: HTMLElement;
  private readonly pool: HTMLDivElement[] = [];
  private readonly separatorPool: HTMLDivElement[] = [];
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
    // Absolute rows, so a separator outside the viewport still answers for its command's badge.
    const isBlank = (absoluteRow: number): boolean => buf.getLine(absoluteRow)?.translateToString(true) === '';
    const segments = stripeSegments(this.marks.stripes(), buf.viewportY, rows);
    segments.forEach((seg, i) => {
      const el = this.elementAt(this.pool, 'stripe', i);
      el.hidden = false;
      el.style.top = `${seg.firstRow * cellHeight}px`;
      el.style.height = `${seg.rowCount * cellHeight}px`;
      el.dataset.exit = seg.exit;
      // The stripe draws the badge only when the badge is not on the separator line.
      el.dataset.agent = String(seg.agent && !badgeOnSeparator(seg.separatorRow, isBlank));
      el.dataset.startVisible = String(seg.startVisible);
    });
    for (let i = segments.length; i < this.pool.length; i++) {
      this.pool[i].hidden = true;
    }

    // A separator sits in the blank row the shell leaves before the prompt, centred there. When that row is not
    // blank (the shell integration is degraded or the gap is off), the line sits on the prompt row's top edge, and
    // its command's badge stays on the stripe.
    const separators = separatorSegments(this.marks.separators(), buf.viewportY, rows);
    separators.forEach((sep, i) => {
      const el = this.elementAt(this.separatorPool, 'separator', i);
      const blank = isBlank(buf.viewportY + sep.row);
      el.hidden = false;
      el.style.top = `${separatorTop(sep.row, cellHeight, blank)}px`;
      el.dataset.agent = String(sep.agent && blank);
    });
    for (let i = separators.length; i < this.separatorPool.length; i++) {
      this.separatorPool[i].hidden = true;
    }
  }

  /** The i-th element of a pool, created on first use and kept for reuse. */
  private elementAt(pool: HTMLDivElement[], className: string, i: number): HTMLDivElement {
    let el = pool[i];
    if (!el) {
      el = document.createElement('div');
      el.className = className;
      this.layer.appendChild(el);
      pool.push(el);
    }
    return el;
  }
}
