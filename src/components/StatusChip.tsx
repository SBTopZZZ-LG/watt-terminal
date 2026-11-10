import { formatCwd } from '../lib/format';
import { GridChip } from './GridChip';
import type { Cwd, InputState, IntegrationStatus, LlmStatus, ObserverStatus } from '../lib/types';

interface Props {
  integration: IntegrationStatus;
  observer: ObserverStatus;
  input: InputState;
  cwd: Cwd | null;
  autoApprove: boolean;
  onToggleAutoApprove: () => void;
  llm: LlmStatus;
  /** Opens the AI settings. The model chip is the button for it. */
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
  const { integration, observer, input, cwd, autoApprove, onToggleAutoApprove, llm, onOpenSettings } = props;
  const modelLabel = llm.enabled ? `model: ${llm.model ?? 'unknown'}` : 'mock agent';
  const modelTitle = `${llm.provider ? `${llm.provider} · ` : ''}${modelLabel}. Open AI settings`;
  return (
    <div className="status-chip" role="status">
      <span className={`chip chip-${integration}`}>{INTEGRATION_LABEL[integration]}</span>
      <span className={`chip chip-${observer}`}>{observer === 'ok' ? 'observer ok' : 'observer desynced'}</span>
      <span className="chip">{INPUT_LABEL[input]}</span>
      <GridChip />
      <button type="button" className="chip chip-model chip-button" title={modelTitle} onClick={onOpenSettings}>
        {modelLabel}
      </button>
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
    </div>
  );
}
