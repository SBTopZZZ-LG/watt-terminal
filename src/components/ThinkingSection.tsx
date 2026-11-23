import { useId } from 'react';
import { THINKING_MODE_OPTIONS } from '../lib/thinking';
import type { ThinkingMode } from '../lib/types';

export interface ThinkingSectionProps {
  /** The saved setting: `collapsed`, `auto` (open while the agent thinks, then closed) or `expanded`. */
  thinkingMode: ThinkingMode;
  /** Applied at once and saved with the UI state. Not part of the AI settings draft, so Save does not apply to it. */
  onThinkingModeChange(mode: ThinkingMode): void;
}

/**
 * The reasoning section of the Terminal settings: whether the model's reasoning blocks in the agent transcript are
 * collapsed, open while thinking then collapsed, or always expanded. An immediate preference, like QueueSection.
 */
export function ThinkingSection({ thinkingMode, onThinkingModeChange }: ThinkingSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-thinking-title`}>
      <h3 className="settings-section-title" id={`${uid}-thinking-title`}>
        Reasoning
      </h3>
      <fieldset className="settings-radios">
        <legend>How reasoning blocks show in the agent</legend>
        {THINKING_MODE_OPTIONS.map((option) => (
          <label key={option.mode} className="settings-check">
            <input
              type="radio"
              name={`${uid}-thinking-mode`}
              value={option.mode}
              checked={thinkingMode === option.mode}
              onChange={() => onThinkingModeChange(option.mode)}
            />
            {option.label}
            <span className="settings-hint"> {option.hint}</span>
          </label>
        ))}
      </fieldset>
      <span className="settings-hint">Applies at once. Reasoning is shown while it streams and is not kept after a reload.</span>
    </section>
  );
}
