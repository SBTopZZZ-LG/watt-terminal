import { useId } from 'react';

export interface SuggestSectionProps {
  /** The saved setting: true asks the model for follow-up prompts after each reply. */
  suggestPrompts: boolean;
  /** Applied at once and saved with the UI state. Not part of the AI settings draft, so Save does not apply to it. */
  onSuggestPromptsChange(on: boolean): void;
}

/**
 * The suggested-prompts section of the AI settings: whether the agent proposes a few follow-up prompts after each reply.
 * An immediate preference, like the Terminal section, so it has no Save and no draft. On by default: it costs one extra
 * model call per reply.
 */
export function SuggestSection({ suggestPrompts, onSuggestPromptsChange }: SuggestSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-suggest-title`}>
      <h3 className="settings-section-title" id={`${uid}-suggest-title`}>
        Suggested prompts
      </h3>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={suggestPrompts}
          aria-describedby={`${uid}-suggest-help`}
          onChange={(e) => onSuggestPromptsChange(e.target.checked)}
        />
        Suggest follow-up prompts after each reply
      </label>
      <span className="settings-hint" id={`${uid}-suggest-help`}>
        Makes one extra model call per reply. On by default. Applies at once.
      </span>
    </section>
  );
}
