import { useEffect, useState } from 'react';
import { gridInfo, term } from '../lib/terminal';
import type { GridInfo } from '../lib/terminal';
import { gridTip } from '../lib/gridTip';

/** Shows xterm's grid as cols x rows. The tooltip has the numbers to report when the text looks clipped. */
export function GridChip() {
  const [info, setInfo] = useState<GridInfo | null>(null);
  useEffect(() => {
    const update = () => setInfo(gridInfo());
    // The DOM grid is resized after xterm's onResize fires, so the widths are read on the next frame.
    const sub = term.onResize(() => requestAnimationFrame(update));
    requestAnimationFrame(update);
    return () => sub.dispose();
  }, []);

  if (info === null) {
    return <span className="chip chip-grid">--</span>;
  }
  return (
    <span className="chip chip-grid" data-tip={gridTip(info)}>
      {`${info.cols}×${info.rows}`}
    </span>
  );
}
