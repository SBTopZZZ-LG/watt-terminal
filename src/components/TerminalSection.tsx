import { useId } from 'react';

export interface TerminalSectionProps {
  /** The saved setting: true hides the command separators and agent labels. */
  hideMarks: boolean;
  /** Applied at once and saved with the UI state. It is not part of the AI settings draft, so Save does not apply to it. */
  onHideMarksChange(hide: boolean): void;
  /** The saved setting: true makes a new terminal join the current chat at once. */
  newTabsVisible: boolean;
  /** Applied at once and saved with the UI state, as hideMarks is. */
  onNewTabsVisibleChange(on: boolean): void;
}

/**
 * The Terminal section of the AI settings: how the terminal shows the agent's work. It is an immediate preference, not part
 * of the settings file, so it has no Save and no draft.
 */
export function TerminalSection({ hideMarks, onHideMarksChange, newTabsVisible, onNewTabsVisibleChange }: TerminalSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-terminal-title`}>
      <h3 className="settings-section-title" id={`${uid}-terminal-title`}>
        Terminal
      </h3>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={hideMarks}
          aria-describedby={`${uid}-marks-help`}
          onChange={(e) => onHideMarksChange(e.target.checked)}
        />
        Hide command separators and agent labels in the terminal
      </label>
      <span className="settings-hint" id={`${uid}-marks-help`}>
        Applies at once. The coloured bars that show each command's exit code stay visible.
      </span>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={newTabsVisible}
          aria-describedby={`${uid}-tabs-help`}
          onChange={(e) => onNewTabsVisibleChange(e.target.checked)}
        />
        New terminals are visible to the agent
      </label>
      <span className="settings-hint" id={`${uid}-tabs-help`}>
        Off: a new tab is private to you until you assign it. On: new tabs join the current chat automatically.
      </span>
    </section>
  );
}
