import { formatCwd } from '../lib/format';
import { chipBrokenTitle, chipLabel, recoveryKind } from '../lib/settingsRecovery';
import { GridChip } from './GridChip';
import type { Cwd, InputState, IntegrationStatus, LlmStatus, ObserverStatus, SettingsStateView } from '../lib/types';

interface Props {
  integration: IntegrationStatus;
  observer: ObserverStatus;
  input: InputState;
  cwd: Cwd | null;
  autoApprove: boolean;
  onToggleAutoApprove: () => void;
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
  const { integration, observer, input, cwd, autoApprove, onToggleAutoApprove, llm, settingsState = null, onOpenSettings } = props;
  const modelLabel = chipLabel(settingsState, llm);
  const broken = recoveryKind(settingsState) !== 'ok';
  const modelTitle = chipBrokenTitle(settingsState) ?? `${llm.provider ? `${llm.provider} · ` : ''}${modelLabel}`;
  return (
    <div className="status-chip" role="status">
      <span className={`chip chip-${integration}`}>{INTEGRATION_LABEL[integration]}</span>
      <span className={`chip chip-${observer}`}>{observer === 'ok' ? 'observer ok' : 'observer desynced'}</span>
      <span className="chip">{INPUT_LABEL[input]}</span>
      <GridChip />
      <span className={`chip chip-model${broken ? ' chip-warn' : ''}`} title={modelTitle}>
        {modelLabel}
      </span>
      <span className="chip chip-cwd" title={cwd ? `${cwd.host}:${cwd.path}` : undefined}>
        {formatCwd(cwd)}
      </span>
      <button
        type="button"
        className={`chip chip-button${autoApprove ? ' chip-warn' : ''}`}
        onClick={onToggleAutoApprove}
        title="Lets the agent run commands without asking. Commands still run only at an idle prompt. Off by default."
      >
        auto-approve: {autoApprove ? 'ON' : 'off'}
      </button>
      <button
        type="button"
        className={`chip chip-button chip-icon${broken ? ' chip-warn' : ''}`}
        aria-label={broken ? 'AI settings (needs attention)' : 'AI settings'}
        title="AI settings (Ctrl/Cmd+,)"
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
