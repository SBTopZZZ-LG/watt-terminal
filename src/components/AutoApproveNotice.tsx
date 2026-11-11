interface Props {
  /** Turns approve-all off. The same toggle as the status strip's chip. */
  onTurnOff(): void;
}

/**
 * The warning at the top of the agent's chat view while approve-all is on: every command runs without a card. It is the same
 * family as SettingsBanner, but it cannot be dismissed. It stays until the user turns approve-all off, or the app restarts.
 */
export function AutoApproveNotice({ onTurnOff }: Props) {
  return (
    <div className="agent-notice" role="status">
      <span className="agent-notice-text">
        Auto-approve is ON: the agent runs every command without asking. It resets when the app restarts.
      </span>
      <button type="button" className="agent-notice-action" onClick={onTurnOff}>
        Turn off
      </button>
    </div>
  );
}
