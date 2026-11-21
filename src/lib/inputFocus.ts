// When the agent input bar may take keyboard focus back. The bar keeps its focus on purpose: while the shell is
// locked (an integration runs) the field is read-only, not disabled, so focus stays put. A focus holder other than the
// bar is accepted only from a blocker: a modal dialog, or a question the agent is waiting on the user to answer. Pure, so
// the rules are tested without the DOM; the real focus behaviour is checked by hand (see InputBar.tsx).

export interface FocusRestoreContext {
  /** The bar's input had focus when the lock began. */
  inputWasFocused: boolean;
  /** Nothing holds focus now: it sits on the page body, or nowhere. Focus on any other element was put there on purpose. */
  focusLost: boolean;
  /** A modal dialog is open (settings, help, the integrate dialog). */
  dialogOpen: boolean;
  /** The agent asked the user a question and the card is waiting for the answer. */
  questionPending: boolean;
}

/** True when the bar should take focus back: it had focus, nothing holds it now, and no blocker is up. */
export function shouldRestoreInputFocus(c: FocusRestoreContext): boolean {
  return c.inputWasFocused && c.focusLost && !c.dialogOpen && !c.questionPending;
}
