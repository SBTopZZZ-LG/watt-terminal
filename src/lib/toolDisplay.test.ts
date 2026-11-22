import { describe, expect, it } from 'vitest';
import { MAX_CAPTION_CHARS, toolCaption, toolCaptionFull, toolPreview, toolPreviewFull, toolRowHead, waitDuration } from './toolDisplay';

describe('toolCaption', () => {
  it('is null when the summary is absent, empty, or not a string', () => {
    expect(toolCaption({})).toBeNull();
    expect(toolCaption({ summary: '' })).toBeNull();
    expect(toolCaption({ summary: '   ' })).toBeNull();
    expect(toolCaption({ summary: 42 })).toBeNull();
    expect(toolCaption({ summary: null })).toBeNull();
    expect(toolCaption(null)).toBeNull();
    expect(toolCaption('a caption')).toBeNull();
  });

  it('keeps a short caption as it is, trimmed', () => {
    expect(toolCaption({ summary: 'Check the working tree' })).toBe('Check the working tree');
    expect(toolCaption({ summary: '  Answer the sudo prompt  ' })).toBe('Answer the sudo prompt');
  });

  it('cuts after ten words and marks the cut', () => {
    const eleven = 'one two three four five six seven eight nine ten eleven';
    expect(toolCaption({ summary: eleven })).toBe('one two three four five six seven eight nine ten…');
    const ten = 'one two three four five six seven eight nine ten';
    expect(toolCaption({ summary: ten })).toBe(ten);
  });

  it('treats newlines as word breaks, so the caption stays one line', () => {
    expect(toolCaption({ summary: 'first line\nsecond line' })).toBe('first line second line');
  });

  it('caps a long word run at ~80 characters', () => {
    const long = 'x'.repeat(100);
    const caption = toolCaption({ summary: long });
    expect(caption).toBe(`${'x'.repeat(79)}…`);
  });

  it('escapes a bidi override the model sent', () => {
    expect(toolCaption({ summary: 'do x‮evil' })).toBe('do x\\u{202E}evil');
  });
});

describe('toolPreview', () => {
  it('a run_command previews the command, capped at 80 characters', () => {
    expect(toolPreview('run_command', { command: 'ls' })).toBe('ls');
    expect(toolPreview('run_command', { command: 'x'.repeat(100) })).toBe(`${'x'.repeat(79)}…`);
  });

  it('a run_command shows newlines as ⏎ on the one detail line', () => {
    expect(toolPreview('run_command', { command: 'a\nb' })).toBe('a⏎b');
  });

  it('a run_command without a command has no preview', () => {
    expect(toolPreview('run_command', {})).toBeNull();
    expect(toolPreview('run_command', { command: '' })).toBeNull();
    expect(toolPreview('run_command', { command: 7 })).toBeNull();
    expect(toolPreview('run_command', null)).toBeNull();
  });

  it('a send_keys previews the decoded input, capped at 60 characters', () => {
    expect(toolPreview('send_keys', { text: 'echo hi', keys: ['Enter'] })).toBe('echo hi⏎');
    expect(toolPreview('send_keys', { text: 'x'.repeat(80) })).toBe(`${'x'.repeat(59)}…`);
  });

  it('a send_keys with nothing typed has no preview', () => {
    expect(toolPreview('send_keys', {})).toBeNull();
  });

  it('a send_keys with only keys previews the key marks', () => {
    expect(toolPreview('send_keys', { keys: ['Shift'] })).toBe('<Shift>');
  });

  it('other tools have no preview', () => {
    expect(toolPreview('get_cwd', {})).toBeNull();
    expect(toolPreview('wait', { command: 'ls' })).toBeNull();
  });
});

describe('toolCaptionFull and toolPreviewFull', () => {
  it('give the whole caption and detail, where the capped ones are cut', () => {
    const summary = 'one two three four five six seven eight nine ten eleven twelve';
    expect(toolCaption({ summary })).toBe('one two three four five six seven eight nine ten…');
    expect(toolCaptionFull({ summary })).toBe(summary);
    expect(toolCaptionFull({ summary: 'a\nb  c' })).toBe('a b c');
    expect(toolCaptionFull({})).toBeNull();
    const command = 'x'.repeat(100);
    expect(toolPreview('run_command', { command })).toBe(`${'x'.repeat(79)}…`);
    expect(toolPreviewFull('run_command', { command })).toBe(command);
    expect(toolPreviewFull('run_command', { command: 'a\nb' })).toBe('a⏎b');
    expect(toolPreviewFull('send_keys', { text: 'x'.repeat(80) })).toBe('x'.repeat(80));
    // Escaped as the row shows it, so the tip cannot say something else.
    expect(toolPreviewFull('run_command', { command: 'rm ‮evil' })).toBe('rm \\u{202E}evil');
    expect(toolPreviewFull('get_cwd', {})).toBeNull();
    expect(toolPreviewFull('run_command', {})).toBeNull();
  });
});

describe('toolRowHead: MCP tools and the skill tool', () => {
  it('shows an MCP tool as server · tool, with its own caption and detail', () => {
    expect(toolRowHead('mcp__you-com__search', { query: 'x', summary: 'Find the docs' })).toEqual({
      label: 'you-com · search',
      caption: 'Find the docs',
      detail: null,
    });
  });

  it('shows the skill tool as skill, captioned with the skill name', () => {
    expect(toolRowHead('skill', { name: 'review-pr', summary: 'Review it' })).toEqual({ label: 'skill', caption: 'review-pr', detail: null });
  });

  it('falls back to the summary for the skill tool when there is no usable name', () => {
    expect(toolRowHead('skill', { name: '  ', summary: 'Review it' })).toEqual({ label: 'skill', caption: 'Review it', detail: null });
    expect(toolRowHead('skill', {})).toEqual({ label: 'skill', caption: null, detail: null });
  });

  it('captions the file tools by the path, glob or pattern they work on', () => {
    expect(toolRowHead('read_file', { path: 'src/main.rs', offset: 10 })).toEqual({ label: 'read_file', caption: 'src/main.rs', detail: null });
    expect(toolRowHead('list_dir', {})).toEqual({ label: 'list_dir', caption: null, detail: null });
    expect(toolRowHead('glob', { pattern: '*.md' })).toEqual({ label: 'glob', caption: '*.md', detail: null });
    expect(toolRowHead('grep', { pattern: 'needle' })).toEqual({ label: 'grep', caption: "'needle'", detail: null });
    expect(toolRowHead('grep', { pattern: 'needle', path: 'src' })).toEqual({ label: 'grep', caption: "'needle' in src", detail: null });
  });

  it('caps and escapes a file tool caption like the other captions', () => {
    const row = toolRowHead('read_file', { path: `${'a'.repeat(120)}‮` });
    expect([...(row.caption ?? '')].length).toBeLessThanOrEqual(MAX_CAPTION_CHARS);
    expect(row.caption).not.toContain('‮');
  });

  it('leaves a name that only looks like an MCP name as it is', () => {
    expect(toolRowHead('mcp__server', {})).toEqual({ label: 'mcp__server', caption: null, detail: null });
  });
});

describe('toolRowHead', () => {
  it('is the bare name with the caption and the detail', () => {
    expect(toolRowHead('run_command', { command: 'git status', summary: 'Check the working tree' })).toEqual({
      label: 'run_command',
      caption: 'Check the working tree',
      detail: 'git status',
    });
  });

  it('leaves the caption and detail null when there is nothing to show', () => {
    expect(toolRowHead('get_cwd', {})).toEqual({ label: 'get_cwd', caption: null, detail: null });
  });
});

describe('waitDuration', () => {
  it('a running wait shows the seconds it was asked for', () => {
    expect(waitDuration('wait', { seconds: 5 }, null, 'running')).toBe('waiting 5s');
  });

  it('a running wait with no seconds shows the backend default (10), and clamps the request to 1..120', () => {
    expect(waitDuration('wait', {}, null, 'running')).toBe('waiting 10s');
    expect(waitDuration('wait', { seconds: 500 }, null, 'running')).toBe('waiting 120s');
    expect(waitDuration('wait', { seconds: 0 }, null, 'running')).toBe('waiting 1s');
    expect(waitDuration('wait', { seconds: 'soon' }, null, 'running')).toBe('waiting 10s');
    expect(waitDuration('wait', null, null, 'running')).toBe('waiting 10s');
  });

  it('a live result shows the time it waited, to one decimal', () => {
    expect(waitDuration('wait', { seconds: 5 }, { reason: 'screen_changed', waited_ms: 4900 }, 'ok')).toBe('waited 4.9s');
    expect(waitDuration('wait', { seconds: 5 }, { waited_ms: 0 }, 'ok')).toBe('waited 0.0s');
  });

  it('a restored result shows the time from the text the model saw', () => {
    expect(waitDuration('wait', { seconds: 5 }, 'waited 4.9s: the screen changed', 'ok')).toBe('waited 4.9s');
    expect(waitDuration('wait', { seconds: 5 }, 'waited 12.0s: the wait time ran out', 'ok')).toBe('waited 12.0s');
  });

  it('falls back to the requested seconds when the result has no usable time', () => {
    expect(waitDuration('wait', { seconds: 7 }, {}, 'ok')).toBe('waited 7s');
    expect(waitDuration('wait', { seconds: 7 }, { waited_ms: 'x' }, 'ok')).toBe('waited 7s');
    expect(waitDuration('wait', { seconds: 7 }, 'error: no screen', 'ok')).toBe('waited 7s');
    expect(waitDuration('wait', {}, null, 'ok')).toBe('waited 10s');
  });

  it('a failed wait has no duration, so its status word stays', () => {
    expect(waitDuration('wait', { seconds: 5 }, 'error: boom', 'failed')).toBeNull();
  });

  it('other tools have no duration', () => {
    expect(waitDuration('run_command', { command: 'ls' }, { waited_ms: 900 }, 'ok')).toBeNull();
    expect(waitDuration('get_screen', {}, null, 'running')).toBeNull();
  });
});
