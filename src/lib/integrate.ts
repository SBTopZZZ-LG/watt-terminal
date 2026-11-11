// The shell integration flow in the UI: which foreground programs can be integrated, what the dialog says for each
// outcome and refusal, and the location chip for the nested level on top. Pure, so the wording is tested without the DOM.
// The backend does the work (core/session.rs integrate_shell): the UI asks, waits, and reports what came back.

import { CHIP_COMMAND_CHARS, truncateCommand } from './foreground';
import type { Foreground, InjectRefusal, InputState, IntegrateOutcome, LevelInfo } from './types';

/** The programs that can take a shell: a remote login, a nested shell, su, a container exec. Not a multiplexer. */
const INTEGRATABLE: readonly string[] = ['ssh', 'shell', 'su', 'container_exec'];

/** The dialog's phases. `closed` shows nothing; the others are the dialog's three screens. */
export type IntegratePhase =
  | { kind: 'closed' }
  | { kind: 'consent' }
  | { kind: 'running' }
  | { kind: 'result'; outcome: IntegrateOutcome };

/** The phases the dialog is shown in. */
export type OpenPhase = Exclude<IntegratePhase, { kind: 'closed' }>;

/** True when the strip offers Integrate: a program that can take a shell is running in the foreground. */
export function canIntegrate(state: InputState, fg: Foreground | null | undefined): boolean {
  return state === 'running' && fg !== null && fg !== undefined && INTEGRATABLE.includes(fg.kind);
}

/** The nested levels, outermost first. The local level is not one of them. */
export function nestedLevels(levels: readonly LevelInfo[]): LevelInfo[] {
  return levels.filter((level) => level.kind !== 'local');
}

const REFUSAL_TEXT: Record<InjectRefusal, string> = {
  not_running: 'Nothing is running in this shell, so there is no shell to integrate. Start the ssh or shell command first.',
  alt_screen: 'A full-screen program is open. Close it first, then try again.',
  no_shell_program:
    'The running program is not a shell, ssh, su or a container exec, so there is no shell to integrate.',
  too_deep: 'This shell is nested too deeply to integrate another level.',
  busy: 'Another shell integration is already in progress.',
};

/** The dialog's text for a refusal. A reason this build does not know gets a generic line. */
export function refusalText(reason: string): string {
  return Object.prototype.hasOwnProperty.call(REFUSAL_TEXT, reason)
    ? REFUSAL_TEXT[reason as InjectRefusal]
    : 'The shell cannot be integrated right now.';
}

/** The result of an integration, as the dialog shows it: the text, and whether it went well. */
export interface OutcomeView {
  tone: 'ok' | 'fail';
  text: string;
}

/** The dialog's text for a finished integration (or a refused one). */
export function outcomeView(outcome: IntegrateOutcome): OutcomeView {
  switch (outcome.outcome) {
    case 'integrated': {
      const who = outcome.user !== null && outcome.host !== null ? ` as ${outcome.user}@${outcome.host}` : '';
      return {
        tone: 'ok',
        text: `Integrated: ${outcome.label} (${outcome.sh}${who}). The agent can now run commands here.`,
      };
    }
    case 'unsupported_shell':
      return { tone: 'fail', text: 'This shell is not supported (needs zsh, or bash 4.4+). Nothing was installed.' };
    case 'timeout':
      return {
        tone: 'fail',
        text: 'The shell did not respond in time. Check that it was at its prompt, then try again.',
      };
    case 'failed': {
      // The hint is the shell's own last line. A trailing full stop is not doubled.
      const hint = outcome.hint.trim().replace(/\.$/, '');
      return {
        tone: 'fail',
        text: hint === '' ? 'Integration did not complete.' : `Integration did not complete: ${hint}.`,
      };
    }
    case 'refused':
      return { tone: 'fail', text: refusalText(outcome.reason) };
  }
}

/**
 * The location chip for the nested level on top: its label, cut to the chip's width, and a title with the full label, the
 * shell and user@host.
 */
export function locationChip(level: LevelInfo): { label: string; title: string } {
  const who = level.user !== null && level.host !== null ? `${level.user}@${level.host}` : (level.host ?? level.user);
  const shell = level.sh ?? 'shell';
  return {
    label: truncateCommand(level.label, CHIP_COMMAND_CHARS),
    title: `${level.label}: ${shell}${who ? ` as ${who}` : ''}. The agent runs commands in this shell.`,
  };
}
