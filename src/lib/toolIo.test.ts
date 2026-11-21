import { describe, expect, it } from 'vitest';
import { MAX_JSON_PREVIEW_CHARS, toolInput, toolJson, toolOutput } from './toolIo';

describe('toolInput', () => {
  it('shows a run_command command as typed, with its line breaks', () => {
    expect(toolInput('run_command', { command: 'cd x\nls -la' })).toBe('cd x\nls -la');
  });

  it('escapes what could hide text, as the approval card does (a bidi override)', () => {
    expect(toolInput('run_command', { command: 'echo ‮evil' })).toBe('echo \\u{202E}evil');
  });

  it('shows the decoded input of send_keys', () => {
    expect(toolInput('send_keys', { text: 'echo hi', keys: ['Enter'] })).toBe('echo hi⏎');
    expect(toolInput('send_keys', { keys: ['Ctrl-C'] })).toBe('^C');
  });

  it('shows other tools as key: value lines, strings verbatim and other values as compact JSON', () => {
    expect(toolInput('get_screen', { lines: 20, summary: 'Look at the screen' })).toBe('lines: 20\nsummary: Look at the screen');
    expect(toolInput('wait', { seconds: 5 })).toBe('seconds: 5');
    expect(toolInput('x', { flags: ['a', 'b'], on: true, none: null })).toBe('flags:\na\nb\non: true\nnone: null');
  });

  it('puts a multi-line string under its key, lines kept', () => {
    expect(toolInput('write_file', { path: 'a.txt', content: 'one\ntwo' })).toBe('path: a.txt\ncontent:\none\ntwo');
  });

  it('is empty when there are no arguments, and "(empty)" for an empty command', () => {
    expect(toolInput('get_cwd', {})).toBe('');
    expect(toolInput('get_cwd', undefined)).toBe('');
    expect(toolInput('run_command', { command: '' })).toBe('(empty)');
  });
});

describe('toolOutput', () => {
  it('is empty while there is no result', () => {
    expect(toolOutput('run_command', null)).toBe('');
    expect(toolOutput('run_command', undefined)).toBe('');
  });

  it('shows a finished run_command: its output, then the exit code and status, then the note', () => {
    const result = {
      outcome: 'finished',
      record: { id: 4, command: 'ls', exit_code: 0, status: 'complete', output: 'a.txt\nb.txt\n' },
      note: 'extra',
    };
    expect(toolOutput('run_command', result)).toBe('a.txt\nb.txt\nexit 0 · complete\nextra');
  });

  it('says the exit is unknown when the record has no exit code, and shows a truncated output', () => {
    const result = { outcome: 'finished', record: { id: 4, exit_code: null, status: 'complete', output: 'x', output_truncated: true } };
    expect(toolOutput('run_command', result)).toBe('x\nexit unknown · complete · output truncated');
  });

  it('shows a running command: the screen tail, then its state and record id', () => {
    const result = { outcome: 'running', reason: 'timeout', record_id: 9, screen_tail: ['$ sleep 9', 'working'], hint: 'h' };
    expect(toolOutput('run_command', result)).toBe('$ sleep 9\nworking\nstill running · timeout · record 9');
  });

  it('shows a run that did not start with its reason', () => {
    expect(toolOutput('run_command', { outcome: 'not_run', reason: 'cancelled' })).toBe('not run: cancelled');
  });

  it('reads a restored run_command result, which is stored as JSON text', () => {
    const stored = JSON.stringify({ id: 2, exit_code: 1, status: 'complete', cwd: '~', output: 'boom' });
    expect(toolOutput('run_command', stored)).toBe('boom\nexit 1 · complete');
  });

  it('shows a restored run_command result that is plain text as it is (a note after the JSON, or an error)', () => {
    const withNote = `${JSON.stringify({ id: 2, output: 'x' })}\nThe shell appears to be waiting.`;
    expect(toolOutput('run_command', withNote)).toBe(withNote);
    expect(toolOutput('run_command', 'error: no shell')).toBe('error: no shell');
  });

  it('shows a wait: how long it waited and why it ended, then the screen rows', () => {
    const result = { reason: 'screen_changed', waited_ms: 1300, state: 'running', screen: { rows: ['a', 'b', '', ''] } };
    expect(toolOutput('wait', result)).toBe('waited 1.3s: the screen changed\na\nb');
  });

  it('shows a wait that finished a command with the command output instead of the screen', () => {
    const result = {
      reason: 'command_finished',
      waited_ms: 4200,
      state: 'prompt',
      screen: { rows: ['$ '] },
      record: { id: 3, exit_code: 0, output: 'done\n' },
    };
    expect(toolOutput('wait', result)).toBe('waited 4.2s: the command finished\ndone');
  });

  it('shows a restored wait text as it is', () => {
    expect(toolOutput('wait', 'waited 4.9s: the wait time ran out and nothing finished')).toBe(
      'waited 4.9s: the wait time ran out and nothing finished',
    );
  });

  it('shows a string result as it is, for any tool', () => {
    expect(toolOutput('request_integration', 'Integrated ssh ubuntu@lidia (zsh).')).toBe('Integrated ssh ubuntu@lidia (zsh).');
    expect(toolOutput('ask_user', 'DENIED: nope')).toBe('DENIED: nope');
  });

  it('shows the fields of another tool object as key: value lines', () => {
    expect(toolOutput('send_keys', { outcome: 'sent', bytes: 6, keys: ['Enter'], state: 'running', screen_tail: ['hello', 'hello'] })).toBe(
      'outcome: sent\nbytes: 6\nkeys: Enter\nstate: running\nscreen_tail:\nhello\nhello',
    );
  });

  it('shows a non-object, non-string result as JSON', () => {
    expect(toolOutput('list_commands', [1, 2])).toBe('[1,2]');
  });
});

describe('toolJson', () => {
  it('pretty-prints the arguments and the raw result', () => {
    expect(toolJson({ command: 'ls' }, { outcome: 'not_run', reason: 'cancelled' })).toBe(
      '{\n  "args": {\n    "command": "ls"\n  },\n  "result": {\n    "outcome": "not_run",\n    "reason": "cancelled"\n  }\n}',
    );
  });

  it('shows a restored JSON-object result parsed, and any other string as stored', () => {
    expect(toolJson({}, '{"id":2}')).toContain('"result": {\n    "id": 2\n  }');
    expect(toolJson({}, 'waited 1.0s: x')).toContain('"result": "waited 1.0s: x"');
  });

  it('cuts a very large JSON view at the limit and marks the cut', () => {
    const text = toolJson({}, 'x'.repeat(MAX_JSON_PREVIEW_CHARS));
    expect(text.endsWith('\n... (cut)')).toBe(true);
    expect(text.length).toBeLessThan(MAX_JSON_PREVIEW_CHARS + 50);
  });
});
