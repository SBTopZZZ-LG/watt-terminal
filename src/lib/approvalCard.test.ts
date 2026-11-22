import { describe, expect, it } from 'vitest';
import { isFileWrite, previewText, showsDisplay } from './approvalCard';

describe('isFileWrite', () => {
  it('is true for the two write tools only', () => {
    expect(isFileWrite('write_file')).toBe(true);
    expect(isFileWrite('edit_file')).toBe(true);
    expect(isFileWrite('read_file')).toBe(false);
    expect(isFileWrite('run_command')).toBe(false);
  });
});

describe('showsDisplay', () => {
  it('shows the display for send_keys, external tools and file writes', () => {
    expect(showsDisplay('send_keys', null)).toBe(true);
    expect(showsDisplay('mcp__a__b', 'external_tool')).toBe(true);
    expect(showsDisplay('write_file', 'fs_write')).toBe(true);
    expect(showsDisplay('edit_file', 'fs_write')).toBe(true);
  });

  it('shows the command for run_command', () => {
    expect(showsDisplay('run_command', 'manual_mode')).toBe(false);
    expect(showsDisplay('run_command', null)).toBe(false);
  });
});

describe('previewText', () => {
  it('keeps readable non-ASCII and tabs for a file write', () => {
    expect(previewText('write_file', 'a—b\n\tc')).toBe('a—b\n\tc');
    expect(previewText('edit_file', 'x → é')).toBe('x → é');
  });

  it('still escapes hidden characters for a file write', () => {
    expect(previewText('write_file', 'a‮b​')).toBe('a\\u{202E}b\\u{200B}');
  });

  it('stays strict for every other card', () => {
    expect(previewText('run_command', 'a—b\tc')).toBe('a\\u{2014}b\\u{9}c');
    expect(previewText('send_keys', 'a—b')).toBe('a\\u{2014}b');
    expect(previewText('mcp__a__b', 'xé')).toBe('x\\u{E9}');
  });
});
