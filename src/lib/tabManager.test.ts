import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { WireEvent } from './types';

type Args = Record<string, unknown>;
const invoke = vi.fn(async (_cmd: string, _args?: Args): Promise<unknown> => undefined);
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Args) => invoke(cmd, args),
  // Keeps its callback, so a test can play the backend's events into the channel.
  Channel: class {
    constructor(public onmessage: (value: unknown) => void) {}
  },
}));
// The controller is not needed here (and would load the terminal library).
vi.mock('./tab', () => ({ TabController: class {} }));

// Imported after the mocks, so tabManager.ts and api.ts take the mocked modules.
const { TabManager } = await import('./tabManager');

/** The channel the attach of `chatId` was given. */
function channelFor(chatId: string): { onmessage: (value: unknown) => void } {
  const call = invoke.mock.calls.find(([cmd, args]) => cmd === 'attach_chat' && args?.chatId === chatId);
  if (call === undefined) {
    throw new Error(`no attach for ${chatId}`);
  }
  return (call[1] as { onEvents: { onmessage: (value: unknown) => void } }).onEvents;
}

const attachCalls = () => invoke.mock.calls.filter(([cmd]) => cmd === 'attach_chat');

describe('TabManager chat channels', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async () => undefined);
  });

  it('attaches a chat once, and reuses the attach for later calls', async () => {
    const manager = new TabManager();
    await manager.ensureChatAttached('c2');
    await manager.ensureChatAttached('c2');
    expect(attachCalls()).toHaveLength(1);
    expect(attachCalls()[0][1]).toMatchObject({ chatId: 'c2' });
  });

  it('tags each event with the chat whose channel carried it', async () => {
    const manager = new TabManager();
    const listener = vi.fn();
    manager.subscribeChat(listener);
    await manager.ensureChatAttached('main');
    await manager.ensureChatAttached('c2');
    const done = { type: 'done' } as unknown as WireEvent;
    channelFor('c2').onmessage(done);
    channelFor('main').onmessage(done);
    expect(listener.mock.calls).toEqual([
      [{ kind: 'event', event: done }, 'c2'],
      [{ kind: 'event', event: done }, 'main'],
    ]);
  });

  it('retries a failed attach on the next call', async () => {
    const manager = new TabManager();
    invoke.mockImplementationOnce(async () => {
      throw new Error('backend down');
    });
    await expect(manager.ensureChatAttached('c2')).rejects.toThrow('backend down');
    await manager.ensureChatAttached('c2');
    expect(attachCalls()).toHaveLength(2);
  });

  it('createChat creates the chat, then attaches it, and resolves with its id', async () => {
    const manager = new TabManager();
    invoke.mockImplementation(async (cmd: string) => (cmd === 'create_chat' ? 'c2' : undefined));
    await expect(manager.createChat()).resolves.toBe('c2');
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['create_chat', 'attach_chat']);
    expect(attachCalls()[0][1]).toMatchObject({ chatId: 'c2' });
  });

  it('a terminal restart attaches every known chat, fresh; a connect reuses the attaches', async () => {
    const manager = new TabManager();
    await manager.ensureChatAttached('main');
    await manager.ensureChatAttached('c2');
    invoke.mockClear();
    // The connect path (fresh false) reuses the default chat's attach and c2's.
    await manager['attachKnownChats'](false);
    expect(invoke).not.toHaveBeenCalled();
    // The restart path (fresh true) attaches the default chat and c2 again.
    await manager['attachKnownChats'](true);
    expect(attachCalls().map(([, args]) => (args as { chatId: string }).chatId)).toEqual(['main', 'c2']);
  });

  it('assign sends the chat id it is given', async () => {
    const manager = new TabManager();
    await manager.assign('t2', true, 'c2');
    expect(invoke.mock.calls[0]).toEqual(['assign_tab_cmd', { chatId: 'c2', sessionId: 't2', on: true }]);
  });

  it('move unassigns the terminal from its chat, assigns it to this one, then reads the owners again', async () => {
    const manager = new TabManager();
    invoke.mockImplementation(async (cmd: string) => (cmd === 'list_tabs' ? [] : undefined));
    await manager.move('t2', 'c2', 'main');
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['assign_tab_cmd', 'assign_tab_cmd', 'list_tabs']);
    expect(invoke.mock.calls[0]).toEqual(['assign_tab_cmd', { chatId: 'c2', sessionId: 't2', on: false }]);
    expect(invoke.mock.calls[1]).toEqual(['assign_tab_cmd', { chatId: 'main', sessionId: 't2', on: true }]);
  });

  it('move shows a refusal (the chat it leaves is busy) and assigns nothing more', async () => {
    const manager = new TabManager();
    const seen: unknown[] = [];
    manager.subscribeChat((msg) => seen.push(msg));
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'assign_tab_cmd') {
        throw 'chat c2 is busy';
      }
      return cmd === 'list_tabs' ? [] : undefined;
    });
    await manager.move('t2', 'c2', 'main');
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['assign_tab_cmd', 'list_tabs']);
    expect(seen).toEqual([{ kind: 'error', message: 'chat c2 is busy' }]);
  });
});
