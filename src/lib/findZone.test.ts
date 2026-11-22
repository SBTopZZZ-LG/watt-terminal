import { describe, expect, it } from 'vitest';
import { findZone } from './findZone';

describe('findZone', () => {
  it('is the chat when focus is in the agent panel (input, transcript, chat dialogs)', () => {
    expect(findZone({ inAgentPanel: true })).toBe('chat');
  });

  it('is the terminal everywhere else, including when nothing is focused', () => {
    expect(findZone({ inAgentPanel: false })).toBe('terminal');
  });
});
