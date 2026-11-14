import { describe, expect, it } from 'vitest';
import { CHIP_COMMAND_CHARS, foregroundChip, isUnintegrated, truncateCommand } from './foreground';
import type { Foreground, ForegroundKind, InputState } from './types';

const UNINTEGRATED: Exclude<ForegroundKind, 'other'>[] = ['ssh', 'shell', 'su', 'container_exec', 'multiplexer'];

describe('isUnintegrated', () => {
  it('is true for every kind but other, and false without a hint', () => {
    for (const kind of UNINTEGRATED) {
      expect(isUnintegrated({ kind, command: 'x' }), kind).toBe(true);
    }
    expect(isUnintegrated({ kind: 'other', command: 'sleep 2' })).toBe(false);
    expect(isUnintegrated(null)).toBe(false);
    expect(isUnintegrated(undefined)).toBe(false);
  });
});

describe('truncateCommand', () => {
  it('keeps the chip limit at 28 characters', () => {
    expect(CHIP_COMMAND_CHARS).toBe(28);
  });

  it('leaves a command that fits as it is, including one of exactly the limit', () => {
    expect(truncateCommand('ssh ubuntu@lidia')).toBe('ssh ubuntu@lidia');
    const exact = 'a'.repeat(28);
    expect(truncateCommand(exact)).toBe(exact);
  });

  it('cuts a longer command to 28 characters, the last one an ellipsis', () => {
    const cut = truncateCommand('ssh -p 2222 deploy@build-farm-7.internal.example');
    expect(cut).toBe('ssh -p 2222 deploy@build-fa…');
    expect([...cut]).toHaveLength(28);
    expect(cut.endsWith('…')).toBe(true);
  });

  it('counts characters, not UTF-16 units', () => {
    // Each of these is one character but two UTF-16 units: a cut must not split a pair.
    const emoji = '🙂'.repeat(30);
    const cut = truncateCommand(emoji);
    expect([...cut]).toHaveLength(28);
    expect(cut).toBe(`${'🙂'.repeat(27)}…`);
  });

  it('honours an explicit limit', () => {
    expect(truncateCommand('abcdef', 4)).toBe('abc…');
    expect(truncateCommand('abcd', 4)).toBe('abcd');
  });
});

describe('foregroundChip', () => {
  const ssh: Foreground = { kind: 'ssh', command: 'ssh ubuntu@lidia' };

  it('is null outside Running, whatever the hint says', () => {
    const states: InputState[] = ['booting', 'prompt', 'submitted', 'continuation', 'alt', 'raw', 'exited'];
    for (const state of states) {
      expect(foregroundChip(state, ssh), state).toBeNull();
    }
  });

  it('is null for an ordinary program or no hint: the chip stays as it is', () => {
    expect(foregroundChip('running', { kind: 'other', command: 'sleep 2' })).toBeNull();
    expect(foregroundChip('running', null)).toBeNull();
    expect(foregroundChip('running', undefined)).toBeNull();
  });

  it('labels a running ssh as RUNNING · the command, with the full command and a meaning in the title', () => {
    expect(foregroundChip('running', ssh)).toEqual({
      label: 'RUNNING · ssh ubuntu@lidia',
      title:
        'ssh ubuntu@lidia: a shell on another machine. It has no shell integration, so the agent can read the screen but cannot run commands here.',
    });
  });

  it('gives each kind its own words in the title', () => {
    const words: Record<Exclude<ForegroundKind, 'other'>, string> = {
      ssh: 'a shell on another machine',
      shell: 'a nested shell',
      su: "another user's shell",
      container_exec: 'a shell in a container',
      multiplexer: 'a terminal multiplexer',
    };
    for (const kind of UNINTEGRATED) {
      const chip = foregroundChip('running', { kind, command: 'cmd' });
      expect(chip?.title, kind).toBe(
        `cmd: ${words[kind]}. It has no shell integration, so the agent can read the screen but cannot run commands here.`,
      );
    }
  });

  it('cuts a long command in the label, and keeps it whole in the title', () => {
    const command = 'docker exec -it some-container-name /bin/bash -l';
    const chip = foregroundChip('running', { kind: 'container_exec', command });
    expect(chip?.label).toBe(`RUNNING · ${truncateCommand(command)}`);
    expect(chip?.label.endsWith('…')).toBe(true);
    expect(chip?.title.startsWith(`${command}: a shell in a container.`)).toBe(true);
  });
});

