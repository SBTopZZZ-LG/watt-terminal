import { createElement } from 'react';
import { describe, expect, it } from 'vitest';
import { nodeText } from './codeText';

describe('nodeText', () => {
  it('returns a string as it is', () => {
    expect(nodeText('ls -la')).toBe('ls -la');
  });

  it('joins text across nested elements, the way highlighted markup is nested', () => {
    const highlighted = [
      createElement('span', { className: 'hljs-keyword' }, 'const'),
      ' x = ',
      createElement('span', null, createElement('b', null, '1')),
    ];
    expect(nodeText(highlighted)).toBe('const x = 1');
  });

  it('keeps numbers and skips null, undefined and booleans', () => {
    expect(nodeText(['a', 2, null, undefined, false, true, 'b'])).toBe('a2b');
  });

  it('keeps inner whitespace and indentation', () => {
    expect(nodeText('if x:\n    return 1\n\n  end')).toBe('if x:\n    return 1\n\n  end');
  });

  it('drops one trailing newline only', () => {
    expect(nodeText('ls\n')).toBe('ls');
    expect(nodeText('ls\n\n')).toBe('ls\n');
    expect(nodeText('ls\r\n')).toBe('ls');
    expect(nodeText('ls')).toBe('ls');
  });

  it('returns an empty string for an empty block', () => {
    expect(nodeText(undefined)).toBe('');
    expect(nodeText(null)).toBe('');
    expect(nodeText('\n')).toBe('');
  });
});
