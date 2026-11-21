import { useId, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { isDeclinedIntegration } from '../lib/integrate';
import { decisionLabel, decisionTooltip } from '../lib/approvalReasons';
import { nextTabId } from '../lib/tabKeys';
import { toolCaptionFull, toolPreviewFull, toolRowHead, waitDuration } from '../lib/toolDisplay';
import { toolInput, toolJson, toolOutput } from '../lib/toolIo';
import type { TranscriptItem } from '../lib/transcript';

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>;

type ToolTab = 'io' | 'json';

const TABS: { id: ToolTab; label: string }[] = [
  { id: 'io', label: 'Input / Output' },
  { id: 'json', label: 'JSON' },
];

/**
 * A tool call's row: a summary (name, caption, decision, status or a wait's duration, and the one-line detail), and the
 * expanded body with two tabs. "Input / Output" is the call as it was sent and what came back, as plain text. "JSON" is
 * the arguments and the raw result. Both panels stay in the DOM; the one not selected is hidden. Uncontrolled, like the
 * details element around it.
 */
export function ToolRow({ item }: { item: ToolItem }) {
  const uid = useId();
  const [tab, setTab] = useState<ToolTab>('io');
  // A declined integration request is its own status, not a failure: the user chose it.
  const declined = isDeclinedIntegration(item.name, item.decision);
  const status = declined ? 'declined' : item.status === 'running' ? 'running' : item.status === 'ok' ? 'ok' : 'failed';
  const head = toolRowHead(item.name, item.args);
  // The caption and the detail are cut for the row. Their tips show the whole text, when the row cut them.
  const captionFull = toolCaptionFull(item.args);
  const captionTip = head.caption !== null && captionFull !== head.caption ? captionFull : null;
  const detailFull = toolPreviewFull(item.name, item.args);
  const duration = waitDuration(item.name, item.args, item.result, item.status);
  // A conversation restored from disk has no start time, and its result is the text the model saw, not the raw result.
  const restored = typeof item.result === 'string' && item.startedAt === undefined;

  const onTabKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const next = nextTabId(TABS.map((t) => t.id), tab, e.key);
    if (next === null) {
      return;
    }
    e.preventDefault();
    setTab(next as ToolTab);
    e.currentTarget.querySelector<HTMLButtonElement>(`[data-tab="${next}"]`)?.focus();
  };

  const output = toolOutput(item.name, item.result);
  const input = toolInput(item.name, item.args);
  const outputPlaceholder = item.status === 'running' && output === '' ? '…waiting for output' : output === '' ? '(no output)' : output;

  return (
    <details className={`tool tool-${status}`}>
      <summary>
        <span className="tool-summary">
          <span className="tool-line">
            {head.label}
            {head.caption !== null && <span className="tool-caption" data-tip={captionTip ?? undefined}>{` · ${head.caption}`}</span>}
            {item.decision !== null && !declined && (
              <span className={`decision decision-${item.decision}`} title={decisionTooltip(item.decision, item.reason) ?? undefined}>
                {` ${decisionLabel(item.decision, item.reason)}`}
              </span>
            )}
            {`: ${duration ?? status}`}
          </span>
          {head.detail !== null && (
            // Not cut in the row: the tip shows only where CSS clips it. Cut in the row: the tip always shows.
            <span className="tool-detail" data-tip={detailFull ?? undefined} data-tip-clip={detailFull === head.detail ? '' : undefined}>
              {head.detail}
            </span>
          )}
        </span>
      </summary>
      <div className="tool-tabs">
        <div role="tablist" aria-label="Tool call view" className="tool-tab-strip" onKeyDown={onTabKeyDown}>
          {TABS.map((t) => {
            const active = t.id === tab;
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                data-tab={t.id}
                id={`${uid}-tab-${t.id}`}
                aria-selected={active}
                aria-controls={`${uid}-panel-${t.id}`}
                tabIndex={active ? 0 : -1}
                className={`tool-tab${active ? ' is-active' : ''}`}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            );
          })}
        </div>
        <div role="tabpanel" id={`${uid}-panel-io`} aria-labelledby={`${uid}-tab-io`} hidden={tab !== 'io'} className="tool-panel">
          <div className="tool-io-label">Input</div>
          <pre>{input === '' ? '(none)' : input}</pre>
          <div className="tool-io-label">Output</div>
          <pre>{outputPlaceholder}</pre>
        </div>
        <div role="tabpanel" id={`${uid}-panel-json`} aria-labelledby={`${uid}-tab-json`} hidden={tab !== 'json'} className="tool-panel">
          <pre>{toolJson(item.args, item.result)}</pre>
          {restored && <div className="tool-io-note">Restored from history: the result is the text the model saw.</div>}
        </div>
      </div>
    </details>
  );
}
