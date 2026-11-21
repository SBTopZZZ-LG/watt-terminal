import type { Dock } from '../lib/panelLayout';

interface AgentActivityToastProps {
  dock: Dock;
  text: string;
  /** The whole line, for the tip. Defaults to `text`, which is then the whole line. */
  full?: string;
  onOpen: () => void;
}

/**
 * The hidden-agent toast: a fixed overlay shown while the agent works and the chat body is hidden. Clicking it
 * opens the chat. The text is already truncated by agentActivity; the CSS clamps it to two lines, and its tip shows
 * the whole line where it is cut. The button's tip names it.
 */
export function AgentActivityToast({ dock, text, full = text, onOpen }: AgentActivityToastProps) {
  return (
    <button type="button" className={`agent-activity dock-${dock}`} aria-live="polite" data-tip="Show the agent" onClick={onOpen}>
      <span className="agent-activity-spinner" aria-hidden="true" />
      <span className="agent-activity-title">Agent is working</span>
      <span className="agent-activity-text" data-tip={full} data-tip-clip={full === text ? '' : undefined}>
        {text}
      </span>
    </button>
  );
}
