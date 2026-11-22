import { describe, expect, it } from 'vitest';
import { isFileWrite, showsDisplay } from './approvalCard';

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
