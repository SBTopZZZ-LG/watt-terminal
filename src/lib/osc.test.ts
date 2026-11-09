import { describe, expect, it } from 'vitest';
import { parseShellPayload } from './osc';

const N = '0123456789abcdef0123456789abcdef';

describe('parseShellPayload', () => {
  it('parses the prompt, continuation and command events with the right nonce', () => {
    expect(parseShellPayload(133, `A;n=${N}`, N)).toEqual({ kind: 'prompt_start' });
    expect(parseShellPayload(133, `P;n=${N};k=s`, N)).toEqual({ kind: 'continuation' });
    expect(parseShellPayload(133, `C;n=${N}`, N)).toEqual({ kind: 'command_start' });
  });

  it('parses the command end with its exit status and pipestatus', () => {
    expect(parseShellPayload(133, `D;n=${N};exit=3;ps=0,3`, N)).toEqual({
      kind: 'command_end',
      exit: 3,
      pipestatus: [0, 3],
    });
    expect(parseShellPayload(133, `D;n=${N};exit=0;ps=0`, N)).toEqual({
      kind: 'command_end',
      exit: 0,
      pipestatus: [0],
    });
    expect(parseShellPayload(133, `D;n=${N};exit=0`, N)).toEqual({ kind: 'command_end', exit: 0, pipestatus: [] });
  });

  it('a D without a readable exit is not an event', () => {
    expect(parseShellPayload(133, `D;n=${N};ps=0`, N)).toBeNull();
    expect(parseShellPayload(133, `D;n=${N};exit=abc`, N)).toBeNull();
  });

  it('parses the Hello on 633 only', () => {
    expect(parseShellPayload(633, `Hello;n=${N};v=1;zsh=5.9;pid=4242`, N)).toEqual({ kind: 'hello' });
    expect(parseShellPayload(133, `Hello;n=${N};v=1`, N)).toBeNull();
    expect(parseShellPayload(633, `E;n=${N};ls`, N)).toBeNull();
  });

  it('ignores a payload whose nonce differs', () => {
    const other = 'ffffffffffffffffffffffffffffffff';
    expect(parseShellPayload(133, `A;n=${other}`, N)).toBeNull();
    expect(parseShellPayload(133, `D;n=${other};exit=0`, N)).toBeNull();
    expect(parseShellPayload(633, `Hello;n=${other};v=1`, N)).toBeNull();
  });

  it('the nonce is compared exactly, not as a prefix', () => {
    expect(parseShellPayload(133, `A;n=${N}0`, N)).toBeNull();
    expect(parseShellPayload(133, `A;n=${N.slice(0, 16)}`, N)).toBeNull();
  });

  it('ignores untagged payloads, and nonce in any field other than field 2', () => {
    expect(parseShellPayload(133, 'A', N)).toBeNull();
    expect(parseShellPayload(133, 'A;', N)).toBeNull();
    expect(parseShellPayload(133, `D;exit=0;n=${N}`, N)).toBeNull();
  });

  it('ignores everything until the nonce is known', () => {
    expect(parseShellPayload(133, `A;n=${N}`, null)).toBeNull();
    expect(parseShellPayload(133, `A;n=`, '')).toBeNull();
  });

  it('ignores event kinds this UI does not use', () => {
    expect(parseShellPayload(133, `B;n=${N}`, N)).toBeNull();
    expect(parseShellPayload(133, `X;n=${N}`, N)).toBeNull();
  });
});
