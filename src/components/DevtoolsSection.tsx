import { useId } from 'react';

export interface DevtoolsSectionProps {
  /** The saved setting: true adds "Open layout inspector" to the right-click menu. */
  devtools: boolean;
  /** Applied at once and saved with the UI state. Not part of the AI settings draft, so Save does not apply to it. */
  onDevtoolsChange(on: boolean): void;
}

/**
 * The developer section of the Terminal tab: whether the right-click menu offers the layout inspector (WebView2 DevTools).
 * An immediate preference, like Notifications, so it has no Save and no draft. Off by default.
 */
export function DevtoolsSection({ devtools, onDevtoolsChange }: DevtoolsSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-devtools-title`}>
      <h3 className="settings-section-title" id={`${uid}-devtools-title`}>
        Developer
      </h3>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={devtools}
          aria-describedby={`${uid}-devtools-help`}
          onChange={(e) => onDevtoolsChange(e.target.checked)}
        />
        Enable the layout inspector
      </label>
      <span className="settings-hint" id={`${uid}-devtools-help`}>
        Adds Open layout inspector to the right-click menu. Applies at once.
      </span>
    </section>
  );
}
