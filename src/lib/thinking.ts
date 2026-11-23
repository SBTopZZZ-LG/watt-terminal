// The reasoning block's pure rules: when it is open, its header label, and the setting's option labels. The labels are
// the single source for the settings UI (ThinkingSection) and the help text, so they are not repeated elsewhere.

import type { ThinkingMode } from './types';

/** The setting's options, in the order the settings show them. */
export const THINKING_MODE_OPTIONS: { mode: ThinkingMode; label: string; hint: string }[] = [
  { mode: 'collapsed', label: 'Collapsed', hint: 'Thinking blocks stay closed; click one to peek.' },
  { mode: 'auto', label: 'Auto', hint: 'Open while the agent is thinking, then collapse.' },
  { mode: 'expanded', label: 'Expanded', hint: 'Always show the full reasoning.' },
];

/** The mode a file without a valid value reads as. */
export const DEFAULT_THINKING_MODE: ThinkingMode = 'auto';

/**
 * Whether a reasoning block is open. A click on the block (`manual`, true or false) overrides the mode for that block;
 * with no click (`manual` null) the mode decides: collapsed, expanded, or open only while the reasoning streams (auto).
 */
export function thinkingOpen(mode: ThinkingMode, streaming: boolean, manual: boolean | null): boolean {
  if (manual !== null) {
    return manual;
  }
  switch (mode) {
    case 'collapsed':
      return false;
    case 'expanded':
      return true;
    case 'auto':
      return streaming;
  }
}

/** The header label once the reasoning has ended: "Thought for 4s". Under a second, or without both times, it is "Thought". */
export function thoughtLabel(startedAt?: number, endedAt?: number): string {
  if (startedAt === undefined || endedAt === undefined) {
    return 'Thought';
  }
  const ms = endedAt - startedAt;
  return ms < 1000 ? 'Thought' : `Thought for ${Math.round(ms / 1000)}s`;
}
