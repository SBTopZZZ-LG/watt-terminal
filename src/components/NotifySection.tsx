import { useId } from 'react';

export interface NotifySectionProps {
  /** The saved setting: true sends a system notification when the agent is done or asks something while Watt is in the background. */
  notifications: boolean;
  /** Applied at once and saved with the UI state. Not part of the AI settings draft, so Save does not apply to it. */
  onNotificationsChange(on: boolean): void;
}

/**
 * The notifications section of the AI settings: whether Watt tells the user, while it is in the background, that the agent
 * is done or needs them. An immediate preference, like the Terminal section, so it has no Save and no draft. On by default.
 */
export function NotifySection({ notifications, onNotificationsChange }: NotifySectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-notify-title`}>
      <h3 className="settings-section-title" id={`${uid}-notify-title`}>
        Notifications
      </h3>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={notifications}
          aria-describedby={`${uid}-notify-help`}
          onChange={(e) => onNotificationsChange(e.target.checked)}
        />
        Notify me when the agent is done or needs me while Watt is in the background
      </label>
      <span className="settings-hint" id={`${uid}-notify-help`}>
        System notification plus a taskbar flash (Windows) or dock bounce (macOS). Click it to bring Watt back. Applies at once.
      </span>
    </section>
  );
}
