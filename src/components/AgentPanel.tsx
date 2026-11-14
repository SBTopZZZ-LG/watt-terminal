import type { ReactNode, Ref } from 'react';
import type { Dock, PanelView } from '../lib/panelLayout';
import wattMark from '../assets/watt-mark.png';

interface WorkspaceProps {
  ref?: Ref<HTMLDivElement>;
  dock: Dock;
  terminal: ReactNode;
  divider: ReactNode;
  panel: ReactNode;
}

/**
 * The terminal and the agent panel side by side (or stacked). The children always come in this order, terminal first, so
 * changing the dock never moves the terminal in the React tree: TerminalView stays mounted and its xterm is not
 * recreated. The dock only sets the class, and App.css turns it into the flex direction (row-reverse for left, column-reverse
 * for top), so the panel sits before the terminal on screen for left and top.
 */
export function Workspace({ ref, dock, terminal, divider, panel }: WorkspaceProps) {
  return (
    <div className={`workspace dock-${dock}`} ref={ref}>
      {terminal}
      {divider}
      {panel}
    </div>
  );
}

interface AgentPanelProps {
  ref?: Ref<HTMLDivElement>;
  view: PanelView;
  /** The rail button: reopens a collapsed side panel. */
  onExpand: () => void;
  /** True while the agent works (a turn or a side command). Shows the rail's busy dot. */
  busy?: boolean;
  /** True while the model has not produced output yet. Kept for the rail's future use. */
  thinking?: boolean;
  /** An approval card waits for an answer. Shows the rail's approval dot. */
  approvalPending?: boolean;
  children: ReactNode;
}

/**
 * The agent panel: the transcript and the input bar, with its size and dock class. On the rail (a closed side panel) the
 * children stay mounted and CSS hides them, so typed text and the transcript's state survive a collapse. The rail button is
 * the only thing shown then. The button sits in the same child slot whether or not the rail is shown.
 */
export function AgentPanel({ ref, view, onExpand, busy = false, approvalPending = false, children }: AgentPanelProps) {
  return (
    <div className={view.panelClass} style={view.style} ref={ref}>
      {view.rail ? (
        <button
          type="button"
          className="agent-rail"
          onClick={onExpand}
          title="Show the agent (Ctrl/Cmd+Shift+A)"
          aria-label="Show the agent panel"
        >
          <img className="brand-mark" src={wattMark} width={212} height={256} alt="" aria-hidden="true" />
          {busy ? (
            <span className="agent-rail-busy" role="img" aria-label="Agent is working" title="Agent is working" />
          ) : null}
          {approvalPending ? (
            <span className="agent-rail-approval" role="img" aria-label="Approval needed" title="Approval needed" />
          ) : null}
          <span className="agent-rail-label">Agent</span>
        </button>
      ) : null}
      {children}
    </div>
  );
}
