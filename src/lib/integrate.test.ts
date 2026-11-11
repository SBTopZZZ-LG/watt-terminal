import { describe, expect, it } from 'vitest';
import {
  agentDialogPhase,
  agentIntegratingQuietly,
  agentRequestAfter,
  agentRequestClosedBySession,
  agentRunning,
  canIntegrate,
  isDeclinedIntegration,
  locationChip,
  nestedLevels,
  outcomeView,
  refusalText,
} from './integrate';
import type { AgentRequest } from './integrate';
import type { Foreground, LevelInfo, WireEvent } from './types';

const fg = (kind: Foreground['kind'], command = 'ssh ubuntu@lidia'): Foreground => ({ kind, command });

const level = (over: Partial<LevelInfo> = {}): LevelInfo => ({
  key: 1,
  kind: 'remote',
  label: 'ssh ubuntu@lidia',
  sh: 'zsh',
  host: 'lidia',
  user: 'ubuntu',
  state: 'prompt',
  ...over,
});

describe('canIntegrate', () => {
  it('offers the integration for a shell, ssh, su or a container exec running in the foreground', () => {
    expect(canIntegrate('running', fg('ssh'))).toBe(true);
    expect(canIntegrate('running', fg('shell', 'zsh -f -i'))).toBe(true);
    expect(canIntegrate('running', fg('su'))).toBe(true);
    expect(canIntegrate('running', fg('container_exec', 'docker exec -it c bash'))).toBe(true);
  });

  it('does not offer it for a multiplexer, an ordinary program, or no hint', () => {
    expect(canIntegrate('running', fg('multiplexer', 'tmux'))).toBe(false);
    expect(canIntegrate('running', fg('other', 'make'))).toBe(false);
    expect(canIntegrate('running', null)).toBe(false);
    expect(canIntegrate('running', undefined)).toBe(false);
  });

  it('does not offer it unless a command is running (at a prompt, or in the alt screen)', () => {
    expect(canIntegrate('prompt', fg('ssh'))).toBe(false);
    expect(canIntegrate('alt', fg('ssh'))).toBe(false);
    expect(canIntegrate('exited', fg('ssh'))).toBe(false);
  });
});

describe('nestedLevels', () => {
  it('drops the local level, and keeps the order', () => {
    const local = level({ key: 0, kind: 'local', label: 'zsh', host: null, user: null, sh: 'zsh' });
    const a = level({ key: 1, label: 'ssh a' });
    const b = level({ key: 2, label: 'ssh b' });
    expect(nestedLevels([local, a, b])).toEqual([a, b]);
    expect(nestedLevels([local])).toEqual([]);
  });
});

describe('outcomeView', () => {
  it('integrated: names the shell, its user and host, and that the agent can run commands', () => {
    expect(
      outcomeView({ outcome: 'integrated', key: 1, label: 'ssh ubuntu@lidia', sh: 'zsh', user: 'ubuntu', host: 'lidia' }),
    ).toEqual({
      tone: 'ok',
      text: 'Integrated: ssh ubuntu@lidia (zsh as ubuntu@lidia). The agent can now run commands here.',
    });
  });

  it('integrated without a user or host: the shell alone', () => {
    expect(outcomeView({ outcome: 'integrated', key: 1, label: 'zsh -f -i', sh: 'bash', user: null, host: null })).toEqual({
      tone: 'ok',
      text: 'Integrated: zsh -f -i (bash). The agent can now run commands here.',
    });
  });

  it('unsupported shell, timeout and refusal are failures with their own text', () => {
    expect(outcomeView({ outcome: 'unsupported_shell' })).toEqual({
      tone: 'fail',
      text: 'This shell is not supported (needs zsh, or bash 4.4+). Nothing was installed.',
    });
    expect(outcomeView({ outcome: 'timeout' })).toEqual({
      tone: 'fail',
      text: 'The shell did not respond in time. Check that it was at its prompt, then try again.',
    });
    expect(outcomeView({ outcome: 'refused', reason: 'busy' })).toEqual({
      tone: 'fail',
      text: 'Another shell integration is already in progress.',
    });
  });

  it('failed: quotes the shell hint, and does not double a full stop', () => {
    expect(outcomeView({ outcome: 'failed', hint: 'agentterm: Unsupported shell (promptvars off)' })).toEqual({
      tone: 'fail',
      text: 'Integration did not complete: agentterm: Unsupported shell (promptvars off).',
    });
    expect(outcomeView({ outcome: 'failed', hint: 'no level appeared.' }).text).toBe(
      'Integration did not complete: no level appeared.',
    );
    expect(outcomeView({ outcome: 'failed', hint: '  ' }).text).toBe('Integration did not complete.');
  });
});

describe('refusalText', () => {
  it('has a line for every refusal the backend sends', () => {
    for (const reason of ['not_running', 'alt_screen', 'no_shell_program', 'too_deep', 'busy']) {
      const text = refusalText(reason);
      expect(text.length).toBeGreaterThan(20);
      expect(text).not.toBe('The shell cannot be integrated right now.');
    }
  });

  it('a reason this build does not know gets the generic line, never the raw code', () => {
    expect(refusalText('something_new')).toBe('The shell cannot be integrated right now.');
    expect(refusalText('constructor')).toBe('The shell cannot be integrated right now.');
  });
});

describe('locationChip', () => {
  it('shows the label, with the shell and user@host in the title', () => {
    expect(locationChip(level())).toEqual({
      label: 'ssh ubuntu@lidia',
      title: 'ssh ubuntu@lidia: zsh as ubuntu@lidia. The agent runs commands in this shell.',
    });
  });

  it('cuts a long label to the chip width, with the full label in the title', () => {
    const long = `ssh ${'a'.repeat(60)}@example.test`;
    const chip = locationChip(level({ label: long }));
    expect([...chip.label].length).toBe(28);
    expect(chip.label.endsWith('…')).toBe(true);
    expect(chip.title.startsWith(long)).toBe(true);
  });

  it('with no sh or user/host, names only what it has', () => {
    expect(locationChip(level({ sh: null, host: null, user: null, label: 'zsh' })).title).toBe(
      'zsh: shell. The agent runs commands in this shell.',
    );
  });
});

// ---- the agent's integration request (request_integration) ----

const INTEGRATED = { outcome: 'integrated' as const, key: 1, label: 'ssh ubuntu@lidia', sh: 'zsh', user: 'ubuntu', host: 'lidia' };
const asked = (callId = 'c1'): WireEvent => ({ type: 'integration_request', call_id: callId, phase: 'asking', label: 'ssh ubuntu@lidia' });
const running = (callId = 'c1'): WireEvent => ({ type: 'integration_request', call_id: callId, phase: 'running', label: 'ssh ubuntu@lidia' });
type AgentOutcome = Extract<WireEvent, { type: 'integration_request' }>['outcome'];
const finished = (callId = 'c1', outcome: AgentOutcome = INTEGRATED): WireEvent => ({
  type: 'integration_request',
  call_id: callId,
  phase: 'done',
  outcome,
});

/** The request after a sequence of events, from nothing. */
function after(events: WireEvent[], start: AgentRequest | null = null): AgentRequest | null {
  return events.reduce<AgentRequest | null>((cur, ev) => agentRequestAfter(cur, ev), start);
}

describe('agentRequestAfter: asked, accepted, finished', () => {
  it('an asking event opens the question with the shell it is about', () => {
    expect(after([asked()])).toEqual({ kind: 'asking', callId: 'c1', label: 'ssh ubuntu@lidia' });
  });

  it('accepted: the running handshake is shown, and its done leaves the result on screen', () => {
    expect(after([asked(), running()])).toEqual({ kind: 'running', callId: 'c1', label: 'ssh ubuntu@lidia', shown: true });
    expect(after([asked(), running(), finished()])).toEqual({ kind: 'result', callId: 'c1', outcome: INTEGRATED });
  });

  it('the user accepting puts the request in the running state before the backend says so: still shown', () => {
    // The UI sets { running, shown } on Integrate; the backend's running event for the same call keeps it shown.
    const onClick: AgentRequest = { kind: 'running', callId: 'c1', label: 'x', shown: true };
    expect(after([running()], { kind: 'asking', callId: 'c1', label: 'x' })).toEqual({ kind: 'running', callId: 'c1', label: 'ssh ubuntu@lidia', shown: true });
    expect(after([running()], onClick)).toEqual({ kind: 'running', callId: 'c1', label: 'ssh ubuntu@lidia', shown: true });
  });

  it('a failed handshake is a result too, with its outcome', () => {
    const failed = { outcome: 'failed' as const, hint: 'agentterm: Unsupported shell' };
    expect(after([asked(), running(), finished('c1', failed)])).toEqual({ kind: 'result', callId: 'c1', outcome: failed });
  });
});

describe('agentRequestAfter: auto mode (no question)', () => {
  it('a running handshake with no question is not shown: the strip says so, and no dialog opens', () => {
    const req = after([running()]);
    expect(req).toEqual({ kind: 'running', callId: 'c1', label: 'ssh ubuntu@lidia', shown: false });
    expect(agentDialogPhase(req)).toBeNull();
    expect(agentIntegratingQuietly(req)).toBe(true);
    expect(agentRunning(req)).toBe(true);
  });

  it('its done clears it: the result is in the transcript, not in a dialog', () => {
    expect(after([running(), finished()])).toBeNull();
  });
});

describe('agentRequestAfter: declined, cut short, or settled by someone else', () => {
  it('a declined done closes the question', () => {
    expect(after([asked(), finished('c1', { outcome: 'declined' } as never)])).toBeNull();
  });

  it('a done for another call leaves the question as it is', () => {
    const cur = after([asked('c1')]);
    expect(after([finished('other')], cur)).toEqual(cur);
  });

  it('a question for a new call replaces the old one', () => {
    expect(after([asked('c1'), asked('c2')])).toEqual({ kind: 'asking', callId: 'c2', label: 'ssh ubuntu@lidia' });
  });

  it('the turn ending (done) closes a question that is still waiting', () => {
    expect(after([asked(), { type: 'done' }])).toBeNull();
  });

  it('the turn ending does not close a handshake that runs or a result on screen', () => {
    expect(after([asked(), running(), { type: 'done' }])).toEqual({ kind: 'running', callId: 'c1', label: 'ssh ubuntu@lidia', shown: true });
    expect(after([asked(), running(), finished(), { type: 'done' }])?.kind).toBe('result');
  });

  it('the shell exiting closes a question that is still waiting, but not a running handshake', () => {
    const exited: WireEvent = { type: 'state', state: 'exited' };
    expect(after([asked(), exited])).toBeNull();
    expect(after([asked(), running(), exited])?.kind).toBe('running');
  });

  it('other events leave the request as it is', () => {
    const cur = after([asked()]);
    expect(agentRequestAfter(cur, { type: 'state', state: 'prompt' })).toBe(cur);
    expect(agentRequestAfter(cur, { type: 'text', text: 'hi' })).toBe(cur);
    expect(agentRequestAfter(null, { type: 'done' })).toBeNull();
  });

  it('a new shell closes a question that is still waiting, and nothing else', () => {
    expect(agentRequestClosedBySession(after([asked()]))).toBeNull();
    const handshake = after([asked(), running()]);
    expect(agentRequestClosedBySession(handshake)).toBe(handshake);
    expect(agentRequestClosedBySession(null)).toBeNull();
  });
});

describe('agentDialogPhase', () => {
  it('a question is the consent screen; a shown handshake is running; a result is the result', () => {
    expect(agentDialogPhase({ kind: 'asking', callId: 'c1', label: 'x' })).toEqual({ kind: 'consent' });
    expect(agentDialogPhase({ kind: 'running', callId: 'c1', label: 'x', shown: true })).toEqual({ kind: 'running' });
    expect(agentDialogPhase({ kind: 'result', callId: 'c1', outcome: INTEGRATED })).toEqual({ kind: 'result', outcome: INTEGRATED });
  });

  it('shows nothing for no request, or a quiet (auto) run', () => {
    expect(agentDialogPhase(null)).toBeNull();
    expect(agentDialogPhase({ kind: 'running', callId: 'c1', label: 'x', shown: false })).toBeNull();
  });
});

describe('agentIntegratingQuietly and agentRunning', () => {
  it('the chip is for the quiet run only; the keys are locked for any run', () => {
    expect(agentIntegratingQuietly({ kind: 'running', callId: 'c1', label: 'x', shown: false })).toBe(true);
    expect(agentIntegratingQuietly({ kind: 'running', callId: 'c1', label: 'x', shown: true })).toBe(false);
    expect(agentIntegratingQuietly({ kind: 'asking', callId: 'c1', label: 'x' })).toBe(false);
    expect(agentIntegratingQuietly(null)).toBe(false);
    expect(agentRunning({ kind: 'running', callId: 'c1', label: 'x', shown: true })).toBe(true);
    expect(agentRunning({ kind: 'running', callId: 'c1', label: 'x', shown: false })).toBe(true);
    expect(agentRunning({ kind: 'asking', callId: 'c1', label: 'x' })).toBe(false);
    expect(agentRunning(null)).toBe(false);
  });
});

describe('isDeclinedIntegration', () => {
  it('is a request_integration the user declined: denied, from the agent tool only', () => {
    expect(isDeclinedIntegration('request_integration', 'denied')).toBe(true);
    expect(isDeclinedIntegration('request_integration', 'manual')).toBe(false);
    expect(isDeclinedIntegration('request_integration', 'refused')).toBe(false);
    expect(isDeclinedIntegration('run_command', 'denied')).toBe(false);
    expect(isDeclinedIntegration('request_integration', null)).toBe(false);
  });
});
