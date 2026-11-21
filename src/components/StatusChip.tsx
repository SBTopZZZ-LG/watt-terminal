import { foregroundChip } from '../lib/foreground';
import { formatCwd } from '../lib/format';
import { shortcutLabel } from '../lib/platform';
import { locationChip } from '../lib/integrate';
import { chipBrokenTitle, chipLabel, recoveryKind } from '../lib/settingsRecovery';
import { GridChip } from './GridChip';
import type { Cwd, Foreground, InputState, IntegrationStatus, LevelInfo, LlmStatus, ObserverStatus, SettingsStateView } from '../lib/types';

interface Props {
  integration: IntegrationStatus;
  observer: ObserverStatus;
  input: InputState;
  /** The foreground hint while a command runs. A program without shell integration replaces the state chip's label. */
  foreground?: Foreground | null;
  cwd: Cwd | null;
  /** The nested level on top, when there is one. Its label is the location chip. */
  location?: LevelInfo | null;
  /** Offers the Integrate button in the right-hand group, just before the gear, so it stays visible in a narrow strip. */
  canIntegrate?: boolean;
  onIntegrate?: () => void;
  /** The agent is integrating this shell without asking (auto mode): the strip says so. The dialog is not shown then. */
  integrating?: boolean;
  llm: LlmStatus;
  /** The state of settings.json. While it cannot be used, the model chip says so, and the settings button is in warning colour. */
  settingsState?: SettingsStateView | null;
  /** Opens the AI settings. The gear button at the end of the strip is the button for it. */
  onOpenSettings: () => void;
}

const INPUT_LABEL: Record<InputState, string> = {
  booting: 'starting',
  prompt: 'prompt',
  submitted: 'submitted',
  running: 'running',
  continuation: 'continuation',
  unresponsive: 'UNRESPONSIVE',
  alt: 'full-screen app',
  raw: 'raw',
  exited: 'exited',
};

const INTEGRATION_LABEL: Record<IntegrationStatus, string> = {
  booting: 'integration: booting',
  live: 'integration: live',
  raw: 'integration: none',
};

/** Slim status strip above the terminal, in normal flow. */
export function StatusChip(props: Props) {
  const {
    integration,
    observer,
    input,
    foreground = null,
    cwd,
    location = null,
    canIntegrate = false,
    onIntegrate,
    integrating = false,
    llm,
    settingsState = null,
    onOpenSettings,
  } = props;
  const fgChip = foregroundChip(input, foreground);
  const loc = location === null ? null : locationChip(location);
  const modelLabel = chipLabel(settingsState, llm);
  const broken = recoveryKind(settingsState) !== 'ok';
  const modelTitle = chipBrokenTitle(settingsState) ?? `${llm.provider ? `${llm.provider} · ` : ''}${modelLabel}`;
  // The provider's name is shown only while the model label shows the model: a broken or last-good state says so instead.
  const providerName = !broken && llm.provider ? llm.provider : null;
  return (
    <div className="status-chip" role="status">
      {/* Shown only when there is something to fix: a live integration and an ok observer are the normal case. */}
      {integration !== 'live' && <span className={`chip chip-${integration}`}>{INTEGRATION_LABEL[integration]}</span>}
      {observer !== 'ok' && <span className={`chip chip-${observer}`}>observer desynced</span>}
      {fgChip ? (
        <span className="chip chip-foreground" data-tip={fgChip.title}>
          {fgChip.label}
        </span>
      ) : (
        <span className="chip chip-state">{INPUT_LABEL[input]}</span>
      )}
      {loc !== null && (
        <span className="chip chip-location" data-tip={loc.title}>
          {loc.label}
        </span>
      )}
      <GridChip />
      {providerName !== null && (
        <span className="chip chip-provider" data-tip={providerName}>
          {`provider: ${providerName}`}
        </span>
      )}
      <span className={`chip chip-model${broken ? ' chip-warn' : ''}`} data-tip={modelTitle}>
        {modelLabel}
      </span>
      <span className="chip chip-cwd" data-tip={cwd ? `${cwd.host}:${cwd.path}` : undefined}>
        {formatCwd(cwd)}
      </span>
      {integrating && (
        <span className="chip chip-integrating" data-tip="The agent is integrating this shell. It was allowed to do so without asking.">
          Integrating…
        </span>
      )}
      {canIntegrate && onIntegrate && (
        <button
          type="button"
          className="chip chip-integrate"
          aria-label="Integrate this shell"
          data-tip="Integrate this shell: the agent can then run commands in it. It asks first."
          onMouseDown={(e) => e.preventDefault()}
          onClick={onIntegrate}
        >
          Integrate
        </button>
      )}
      <button
        type="button"
        className={`chip chip-button chip-icon${broken ? ' chip-warn' : ''}`}
        aria-label={broken ? 'AI settings (needs attention)' : 'AI settings'}
        data-tip={`AI settings (${shortcutLabel('settings')})`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onOpenSettings}
      >
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.25" aria-hidden="true">
          <path d="M7.04 3.19L7.35 1.63L8.65 1.63L8.96 3.19L10.72 3.93L12.04 3.04L12.96 3.96L12.07 5.28L12.81 7.04L14.37 7.35L14.37 8.65L12.81 8.96L12.07 10.72L12.96 12.04L12.04 12.96L10.72 12.07L8.96 12.81L8.65 14.37L7.35 14.37L7.04 12.81L5.28 12.07L3.96 12.96L3.04 12.04L3.93 10.72L3.19 8.96L1.63 8.65L1.63 7.35L3.19 7.04L3.93 5.28L3.04 3.96L3.96 3.04L5.28 3.93Z" />
          <circle cx="8" cy="8" r="2.2" />
        </svg>
        {broken && <span className="chip-dot" aria-hidden="true" />}
      </button>
    </div>
  );
}
