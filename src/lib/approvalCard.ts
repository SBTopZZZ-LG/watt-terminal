// What an approval card shows as its body. A send_keys card shows its decoded input, an external tool its `server: tool`, and a
// file write (write_file, edit_file) its preview from the backend (core/fs_tools/write.rs). The other cards show the command,
// with newlines as ⏎. Pure, so the choice is tested without the DOM.

import type { ApprovalReasonCode } from './types';
import { visibleEscapes, visiblePreviewEscapes } from './visibleEscapes';

/** Whether `name` is a file write tool (core/fs_tools): its card shows the preview, in a block that keeps its lines. */
export function isFileWrite(name: string): boolean {
  return name === 'write_file' || name === 'edit_file';
}

/** Whether the card shows its `display` instead of the command: send_keys, an external tool, or a file write. */
export function showsDisplay(name: string, reason: ApprovalReasonCode | null): boolean {
  return name === 'send_keys' || reason === 'external_tool' || isFileWrite(name);
}

/**
 * The card's text, escaped for `name`: a file write's preview keeps readable non-ASCII (visiblePreviewEscapes); every
 * other card stays strict (visibleEscapes).
 */
export function previewText(name: string, text: string): string {
  return isFileWrite(name) ? visiblePreviewEscapes(text) : visibleEscapes(text);
}
