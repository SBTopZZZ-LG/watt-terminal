import { useId } from 'react';

export interface TerminalSectionProps {
  /** The saved setting: true hides the command separators and agent labels. */
  hideMarks: boolean;
  /** Applied at once and saved with the UI state. It is not part of the AI settings draft, so Save does not apply to it. */
  onHideMarksChange(hide: boolean): void;
}

/**
 * The Terminal section of the AI settings: how the terminal shows the agent's work. It is an immediate preference, not part
 * of the settings file, so it has no Save and no draft.
 */
export function TerminalSection({ hideMarks, onHideMarksChange }: TerminalSectionProps) {
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
    </section>
  );
}
