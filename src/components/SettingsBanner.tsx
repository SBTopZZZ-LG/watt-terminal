import { bannerText } from '../lib/settingsRecovery';
import type { SettingsStateView } from '../lib/types';

interface Props {
  state: SettingsStateView;
  onOpenSettings(): void;
  onDismiss(): void;
}

/**
 * The notice at the top of the agent's chat view while settings.json cannot be used. It says what that means for the agent,
 * opens the settings, and can be dismissed. Its text is built by settingsRecovery.ts, from the reason safeText has cleaned.
 */
export function SettingsBanner({ state, onOpenSettings, onDismiss }: Props) {
  const text = bannerText(state);
  if (text === null) {
    return null;
  }
  return (
    <div className="agent-notice" role="status">
      <span className="agent-notice-text">{text}</span>
      <button type="button" className="agent-notice-action" onClick={onOpenSettings}>
        Open settings
      </button>
      <button type="button" className="agent-notice-dismiss" onClick={onDismiss}>
        Dismiss
      </button>
    </div>
  );
}
