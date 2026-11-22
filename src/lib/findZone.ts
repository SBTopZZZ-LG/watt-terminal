// Which find Ctrl+F (Cmd+F) opens. Pure: App.tsx reads the focused element and passes whether it is in the agent panel.
// The chat find is for the agent panel (its input, the transcript, the dialogs of the chat). Anywhere else is the terminal.

export type FindZone = 'chat' | 'terminal';

/** The focus, as far as the zone cares. */
export interface FindZoneFocus {
  /** The focused element is inside the agent panel (.agent-panel). */
  inAgentPanel: boolean;
}

/** The find zone for the current focus. */
export function findZone(focus: FindZoneFocus): FindZone {
  return focus.inAgentPanel ? 'chat' : 'terminal';
}
