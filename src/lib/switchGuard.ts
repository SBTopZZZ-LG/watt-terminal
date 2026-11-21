// Whether the live conversation may be switched (new, open) right now. Pure, so the rule is tested without React. The backend
// refuses a switch while the agent is busy; this stops the UI from asking, and from sending into a switch in progress.

export interface SwitchState {
  /** A turn is running (the transcript is busy). */
  busy: boolean;
  /** A side command (`/btw`, `/compact`, `/fork`, `/rename` without a title) is running. */
  sideBusy: boolean;
  /** An open or a new conversation is in flight (the App's `switching` ref). */
  switching: boolean;
}

/** The tooltip of a control that a busy agent blocks (the New button, the History rows and delete buttons). */
export const BUSY_TIP = 'Wait for the agent to finish, or cancel it with Ctrl-C, first';
export const SIDE_BUSY_NOTICE ='A side command is still running. Wait for it to finish, or stop it first.';
export const BUSY_NOTICE = 'The agent is busy. Wait for the turn to finish, or stop it first.';
export const SWITCHING_NOTICE = 'A conversation is already being opened or cleared. Try again in a moment.';
/** A send while a switch is in flight: refused, so the text is kept for the user to send again. */
export const SEND_WHILE_SWITCHING_NOTICE = 'A conversation is being opened or cleared. Send again in a moment.';

/** Why the conversation cannot be switched now, or null when it can. */
export function switchRefusal(s: SwitchState): string | null {
  if (s.sideBusy) {
    return SIDE_BUSY_NOTICE;
  }
  if (s.busy) {
    return BUSY_NOTICE;
  }
  if (s.switching) {
    return SWITCHING_NOTICE;
  }
  return null;
}
