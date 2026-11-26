import { useContext, useEffect, useState } from 'react';
import { ActiveTabContext } from '../lib/activeTab';
import type { GridInfo } from '../lib/tab';
import { gridTip } from '../lib/gridTip';

/** Shows xterm's grid as cols x rows. The tooltip has the numbers to report when the text looks clipped. */
export function GridChip() {
  const tab = useContext(ActiveTabContext);
  const [info, setInfo] = useState<GridInfo | null>(null);
  useEffect(() => {
    if (tab === null) {
      return;
    }
    const update = () => setInfo(tab.gridInfo());
    // The DOM grid is resized after xterm's onResize fires, so the widths are read on the next frame.
    const sub = tab.term.onResize(() => requestAnimationFrame(update));
    requestAnimationFrame(update);
    return () => sub.dispose();
  }, [tab]);

  if (info === null) {
    return <span className="chip chip-grid">--</span>;
  }
  return (
    <span className="chip chip-grid" data-tip={gridTip(info)}>
      {`${info.cols}×${info.rows}`}
    </span>
  );
}
