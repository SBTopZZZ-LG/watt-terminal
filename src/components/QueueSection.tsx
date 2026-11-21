import { useId } from 'react';
import type { QueueMode } from '../lib/types';

export interface QueueSectionProps {
  /** The saved setting: `all` sends the queued messages together after the turn, `one` one per turn. */
  queueMode: QueueMode;
  /** Applied at once and saved with the UI state. Not part of the AI settings draft, so Save does not apply to it. */
  onQueueModeChange(mode: QueueMode): void;
}

/**
 * The queued-messages section of the AI settings: how messages typed while the agent works are sent. An immediate
 * preference, like the Terminal section, so it has no Save and no draft.
 */
export function QueueSection({ queueMode, onQueueModeChange }: QueueSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-queue-title`}>
      <h3 className="settings-section-title" id={`${uid}-queue-title`}>
        Queued messages
      </h3>
      <fieldset className="settings-radios" aria-describedby={`${uid}-queue-help`}>
        <legend>When messages are queued while the agent works</legend>
        <label className="settings-check">
          <input
            type="radio"
            name={`${uid}-queue-mode`}
            value="all"
            checked={queueMode === 'all'}
            onChange={() => onQueueModeChange('all')}
          />
          Send them together, as one message (default)
        </label>
        <label className="settings-check">
          <input
            type="radio"
            name={`${uid}-queue-mode`}
            value="one"
            checked={queueMode === 'one'}
            onChange={() => onQueueModeChange('one')}
          />
          Send them one at a time, one per turn
        </label>
      </fieldset>
      <span className="settings-hint" id={`${uid}-queue-help`}>
        Applies at once. A queued message can be removed before it is sent, and its text goes back into the input.
      </span>
    </section>
  );
}
