// The hidden-approval toast: a fixed overlay shown while an approval card waits and the chat body is hidden.
// Approve and Deny answer the card; Show opens the chat without answering it. The command is the same truncated,
// escaped preview the tool rows show (newlines as ⏎); a send_keys toast shows the decoded input instead.

import { capDisplay } from '../lib/keysDisplay';
import type { Dock } from '../lib/panelLayout';
import { MAX_COMMAND_PREVIEW_CHARS, MAX_SEND_KEYS_PREVIEW_CHARS } from '../lib/toolDisplay';
import type { TranscriptItem } from '../lib/transcript';

type ApprovalItem = Extract<TranscriptItem, { kind: 'approval' }>;

interface ApprovalToastProps {
  approval: ApprovalItem;
  dock: Dock;
  onApprove(callId: string, approve: boolean): void;
  onOpen(): void;
}

export function ApprovalToast({ approval, dock, onApprove, onOpen }: ApprovalToastProps) {
  const command =
    approval.name === 'send_keys'
      ? capDisplay(approval.display ?? approval.text, MAX_SEND_KEYS_PREVIEW_CHARS)
      : capDisplay(approval.text.replace(/\r\n|\n|\r/g, '⏎'), MAX_COMMAND_PREVIEW_CHARS);
  return (
    <div className={`approval-toast dock-${dock}`} role="alertdialog" aria-label="Approval needed">
      <div className="approval-toast-title">Approval needed</div>
      <code className="approval-toast-command">{command}</code>
      <div className="approval-toast-actions">
        <button type="button" className="approve" aria-label="Approve" onClick={() => onApprove(approval.callId, true)}>
          Approve
        </button>
        <button type="button" className="deny" aria-label="Deny" onClick={() => onApprove(approval.callId, false)}>
          Deny
        </button>
        <button type="button" className="show" aria-label="Show the approval" onClick={onOpen}>
          Show
        </button>
      </div>
    </div>
  );
}
