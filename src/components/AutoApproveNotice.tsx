interface Props {
  /** Turns approve-all off. The same toggle as the status strip's chip. */
  onTurnOff(): void;
  /**
   * Approve-all is on, but it was switched on in another shell level, so it does not apply to this one. The notice says so,
   * and its button turns approve-all on for this shell instead.
   */
  suspended?: boolean;
  onTurnOnHere?(): void;
}

/**
 * The warning at the top of the agent's chat view while approve-all is on: every command runs without a card. It is the same
 * family as SettingsBanner, but it cannot be dismissed. It stays until the user turns approve-all off, or the app restarts.
 * Suspended, the warning says approve-all is off in this shell, and its button turns it on for this one.
 */
export function AutoApproveNotice({ onTurnOff, suspended = false, onTurnOnHere }: Props) {
  if (suspended) {
    return (
      <div className="agent-notice" role="status">
        <span className="agent-notice-text">
          Auto-approve was turned on in another shell, so it is off in this one: every command here asks for your approval.
        </span>
        <button type="button" className="agent-notice-action" onClick={onTurnOnHere}>
          Turn on here
        </button>
      </div>
    );
  }
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
