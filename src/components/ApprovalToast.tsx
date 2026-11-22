// The hidden-approval toast: a fixed overlay shown while an approval card waits and the chat body is hidden.
// Approve and Deny answer the card; Show opens the chat without answering it. The command is the same truncated,
// escaped preview the tool rows show (newlines as ⏎); a send_keys toast shows the decoded input instead, and a file write
// shows its preview in a block that keeps the lines.

import { isFileWrite, previewText, showsDisplay } from '../lib/approvalCard';
import { capDisplay, displayText } from '../lib/keysDisplay';
import type { Dock } from '../lib/panelLayout';
import { MAX_COMMAND_PREVIEW_CHARS, MAX_SEND_KEYS_PREVIEW_CHARS } from '../lib/toolDisplay';
import type { TranscriptItem } from '../lib/transcript';
import { ApprovalCountdown } from './ApprovalCountdown';

type ApprovalItem = Extract<TranscriptItem, { kind: 'approval' }>;

interface ApprovalToastProps {
  approval: ApprovalItem;
  dock: Dock;
  onApprove(callId: string, approve: boolean): void;
  onOpen(): void;
}

export function ApprovalToast({ approval, dock, onApprove, onOpen }: ApprovalToastProps) {
  const sendKeys = approval.name === 'send_keys';
  // A send_keys card shows its decoded input, an external tool its `server: tool`, a file write its preview; the other cards
  // show the command.
  const shownInstead = showsDisplay(approval.name, approval.reason);
  const preview = shownInstead ? (approval.display ?? approval.text) : approval.text.replace(/\r\n|\n|\r/g, '⏎');
  const max = sendKeys ? MAX_SEND_KEYS_PREVIEW_CHARS : MAX_COMMAND_PREVIEW_CHARS;
  const command = capDisplay(preview, max);
  // The tip is the whole preview, escaped the same way, so it cannot say something the command does not. A cut preview
  // always shows its tip; an uncut one shows it only where CSS clips it.
  return (
    <div className={`approval-toast dock-${dock}`} role="alertdialog" aria-label="Approval needed">
      <div className="approval-toast-title">Approval needed</div>
      {isFileWrite(approval.name) ? (
        <pre className="approval-toast-preview">{previewText(approval.name, preview)}</pre>
      ) : (
        <code className="approval-toast-command" data-tip={displayText(preview)} data-tip-clip={[...preview].length > max ? undefined : ''}>
          {command}
        </code>
      )}
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
      <ApprovalCountdown expiresAt={approval.expiresAt} />
    </div>
  );
}
