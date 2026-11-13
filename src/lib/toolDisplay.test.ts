import { describe, expect, it } from 'vitest';
import { toolCaption, toolPreview, toolRowHead } from './toolDisplay';

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
