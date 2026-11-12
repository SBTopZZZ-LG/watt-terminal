import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import type { ContextUsage } from '../lib/types';
import { detailMaxHeight, formatTokens, groupDigits, headline, level, percent, rows, segments, share } from '../lib/contextMeter';

/** The stacked bar's colours repeat after this many categories. Matches .context-seg-0 to .context-seg-6 in App.css. */
const SLOTS = 7;

interface Props {
  /** The latest request's usage. Null hides the meter altogether. */
  usage: ContextUsage | null;
}

/**
 * The expanded view's contents: the headline, whether the count is the provider's or an estimate, the stacked bar (only when
 * the limit is known), one row per category with Free and Reserved, and a line when older content was left out.
 */
export function ContextBreakdown({ usage }: { usage: ContextUsage }) {
  const limit = usage.limit !== null && usage.limit > 0 ? usage.limit : null;
  return (
    <>
      <p className="context-headline">{headline(usage)}</p>
      <p className="context-basis">{usage.calibrated ? "Provider's count" : 'Estimated'}</p>
      {limit !== null && (
        <div className="context-stack" aria-hidden="true">
          {segments(usage).map((s, i) => (
            <span key={i} className={`context-seg context-seg-${i % SLOTS}`} style={{ width: `${s.share}%` }} />
          ))}
        </div>
      )}
      <ul className="context-rows">
        {rows(usage).map((r, i) => (
          <li key={i} className={`context-row context-row-${r.kind}`}>
            <span className={r.slot === null ? 'context-swatch' : `context-swatch context-seg-${r.slot % SLOTS}`} aria-hidden="true" />
            <span className="context-label">{r.label}</span>
            <span className="context-tokens">{groupDigits(r.tokens)}</span>
            <span className="context-percent">{r.percent}</span>
          </li>
        ))}
      </ul>
      {usage.trimmed && <p className="context-trimmed">Older content was left out of the last request.</p>}
    </>
  );
}

/**
 * How full the model's context is, above the input bar (the numbers are in contextMeter.ts). Collapsed, it is one button: a
 * slim bar, the percentage, and the tokens when the panel is wide enough (CSS decides; see .context-meter-figures). Expanded,
 * the breakdown opens in the flow above the button, so nothing is clipped. Esc closes it and returns focus to the button.
 * A click elsewhere leaves it open. Its open state is local and is kept while the usage updates.
 */
export function ContextMeter({ usage }: Props) {
  const [open, setOpen] = useState(false);
  const [cap, setCap] = useState(detailMaxHeight(0));
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const detailId = useId();
  const shown = usage !== null;

  // The breakdown's height follows the agent panel: 60% of it, at most 240px.
  useLayoutEffect(() => {
    const agent = wrapRef.current?.closest<HTMLElement>('.agent-panel');
    if (!agent || typeof ResizeObserver === 'undefined') {
      return;
    }
    const measure = () => setCap(detailMaxHeight(agent.clientHeight));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(agent);
    return () => observer.disconnect();
  }, [shown]);

  if (usage === null) {
    return null;
  }

  const limit = usage.limit !== null && usage.limit > 0 ? usage.limit : null;
  const lvl = level(usage);
  const used = formatTokens(usage.used);
  const summary =
    limit === null
      ? `Context: ${used} tokens used, limit unknown`
      : `Context: ${percent(usage.used, limit)} used, ${used} of ${formatTokens(limit)} tokens`;
  const fill = limit === null ? 0 : Math.min(100, share(usage.used, limit));
  const action = open ? 'Hide' : 'Show';
  const title = limit === null ? `Context limit unknown. ${action} the details` : `${action} the context details`;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    }
  };

  return (
    <div ref={wrapRef} className="context-meter-wrap" onKeyDown={onKeyDown}>
      {/* Always in the DOM, so the button's aria-controls always points at it. Hidden while collapsed. */}
      <div id={detailId} className="context-detail" role="region" aria-label="Context breakdown" hidden={!open} style={{ maxHeight: cap }}>
        <ContextBreakdown usage={usage} />
      </div>
      <button
        ref={buttonRef}
        type="button"
        className={`context-meter is-${lvl}`}
        aria-expanded={open}
        aria-controls={detailId}
        aria-label={summary}
        title={title}
        onClick={() => setOpen((o) => !o)}
      >
        <span
          className="context-meter-bar"
          role={limit === null ? undefined : 'meter'}
          aria-label={limit === null ? undefined : 'Context used'}
          aria-valuemin={limit === null ? undefined : 0}
          aria-valuemax={limit === null ? undefined : 100}
          aria-valuenow={limit === null ? undefined : Math.round(fill)}
        >
          {limit !== null && <span className="context-meter-fill" style={{ width: `${fill}%` }} />}
        </span>
        <span className="context-meter-percent">{limit === null ? used : percent(usage.used, limit)}</span>
        {limit !== null && <span className="context-meter-figures">{`${used} / ${formatTokens(limit)}`}</span>}
        <svg className="context-chevron" viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
    </div>
  );
}
