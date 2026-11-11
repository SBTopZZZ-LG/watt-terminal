import { describe, expect, it } from 'vitest';
import { canIntegrate, locationChip, nestedLevels, outcomeView, refusalText } from './integrate';
import type { Foreground, LevelInfo } from './types';

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
