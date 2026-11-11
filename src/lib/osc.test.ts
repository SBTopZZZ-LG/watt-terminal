import { describe, expect, it } from 'vitest';
import { parseShellPayload } from './osc';

const N = '0123456789abcdef0123456789abcdef';
const ONLY_N = new Set([N]);

describe('parseShellPayload', () => {
  it('parses the prompt, continuation and command events with the right nonce', () => {
    expect(parseShellPayload(133, `A;n=${N}`, ONLY_N)).toEqual({ kind: 'prompt_start' });
    expect(parseShellPayload(133, `P;n=${N};k=s`, ONLY_N)).toEqual({ kind: 'continuation' });
    expect(parseShellPayload(133, `C;n=${N}`, ONLY_N)).toEqual({ kind: 'command_start' });
  });

  it('parses the command end with its exit status and pipestatus', () => {
    expect(parseShellPayload(133, `D;n=${N};exit=3;ps=0,3`, ONLY_N)).toEqual({
      kind: 'command_end',
      exit: 3,
      pipestatus: [0, 3],
    });
    expect(parseShellPayload(133, `D;n=${N};exit=0;ps=0`, ONLY_N)).toEqual({
      kind: 'command_end',
      exit: 0,
      pipestatus: [0],
    });
    expect(parseShellPayload(133, `D;n=${N};exit=0`, ONLY_N)).toEqual({ kind: 'command_end', exit: 0, pipestatus: [] });
  });

  it('a D without a readable exit is not an event', () => {
    expect(parseShellPayload(133, `D;n=${N};ps=0`, ONLY_N)).toBeNull();
    expect(parseShellPayload(133, `D;n=${N};exit=abc`, ONLY_N)).toBeNull();
  });

  it('parses the Hello on 633 only', () => {
    expect(parseShellPayload(633, `Hello;n=${N};v=1;zsh=5.9;pid=4242`, ONLY_N)).toEqual({ kind: 'hello' });
    expect(parseShellPayload(133, `Hello;n=${N};v=1`, ONLY_N)).toBeNull();
    expect(parseShellPayload(633, `E;n=${N};ls`, ONLY_N)).toBeNull();
  });

  it('ignores a payload whose nonce is not accepted', () => {
    const other = 'ffffffffffffffffffffffffffffffff';
    expect(parseShellPayload(133, `A;n=${other}`, ONLY_N)).toBeNull();
    expect(parseShellPayload(133, `D;n=${other};exit=0`, ONLY_N)).toBeNull();
    expect(parseShellPayload(633, `Hello;n=${other};v=1`, ONLY_N)).toBeNull();
  });

  it('the nonce is compared exactly, not as a prefix', () => {
    expect(parseShellPayload(133, `A;n=${N}0`, ONLY_N)).toBeNull();
    expect(parseShellPayload(133, `A;n=${N.slice(0, 16)}`, ONLY_N)).toBeNull();
  });

  it('ignores untagged payloads, and nonce in any field other than field 2', () => {
    expect(parseShellPayload(133, 'A', ONLY_N)).toBeNull();
    expect(parseShellPayload(133, 'A;', ONLY_N)).toBeNull();
    expect(parseShellPayload(133, `D;exit=0;n=${N}`, ONLY_N)).toBeNull();
  });

  it('ignores everything until a nonce is known', () => {
    expect(parseShellPayload(133, `A;n=${N}`, new Set())).toBeNull();
    expect(parseShellPayload(133, 'A;n=', new Set(['']))).toBeNull();
  });

  it('ignores event kinds this UI does not use', () => {
    expect(parseShellPayload(133, `B;n=${N}`, ONLY_N)).toBeNull();
    expect(parseShellPayload(133, `X;n=${N}`, ONLY_N)).toBeNull();
  });

  it('accepts events from every nonce in the set, and no others', () => {
    const level1 = '11111111111111111111111111111111';
    const level2 = '22222222222222222222222222222222';
    const unknown = '33333333333333333333333333333333';
    const set = new Set([N, level1, level2]);
    expect(parseShellPayload(133, `A;n=${N}`, set)).toEqual({ kind: 'prompt_start' });
    expect(parseShellPayload(133, `C;n=${level1}`, set)).toEqual({ kind: 'command_start' });
    expect(parseShellPayload(133, `D;n=${level2};exit=2`, set)).toEqual({ kind: 'command_end', exit: 2, pipestatus: [] });
    expect(parseShellPayload(133, `A;n=${unknown}`, set)).toBeNull();
    expect(parseShellPayload(633, `Hello;n=${unknown};v=1`, set)).toBeNull();
  });

  it('a nonce that has been retired (not in the set any more) is ignored', () => {
    const level1 = '11111111111111111111111111111111';
    const after = new Set([N]);
    expect(parseShellPayload(133, `A;n=${level1}`, new Set([N, level1]))).toEqual({ kind: 'prompt_start' });
    expect(parseShellPayload(133, `A;n=${level1}`, after)).toBeNull();
  });
});
