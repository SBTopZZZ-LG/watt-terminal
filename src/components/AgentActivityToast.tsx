import type { Dock } from '../lib/panelLayout';

interface AgentActivityToastProps {
  dock: Dock;
  text: string;
  onOpen: () => void;
}

/**
 * The hidden-agent toast: a fixed overlay shown while the agent works and the chat body is hidden. Clicking it
 * opens the chat. The text is already truncated by agentActivityText; the CSS clamps it to two lines.
 */
export function AgentActivityToast({ dock, text, onOpen }: AgentActivityToastProps) {
  return (
    <button type="button" className={`agent-activity dock-${dock}`} aria-live="polite" title="Show the agent" onClick={onOpen}>
      <span className="agent-activity-spinner" aria-hidden="true" />
      <span className="agent-activity-title">Agent is working</span>
      <span className="agent-activity-text">{text}</span>
    </button>
  );
}
