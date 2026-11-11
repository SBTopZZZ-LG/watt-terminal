import { beforeEach, describe, expect, it, vi } from 'vitest';

type Args = Record<string, unknown>;
const invoke = vi.fn(async (_cmd: string, _args?: Args) => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: Args) => invoke(cmd, args), Channel: class {} }));

// Imported after the mock, so api.ts takes the mocked invoke.
const { api } = await import('./api');

const IMAGE = { name: 'shot.png', mime: 'image/png', kind: 'image' as const, data_b64: 'iVBORw0K', thumb: 'data:image/jpeg;base64,AAAA', w: 10, h: 20 };

describe('api.agentMessage', () => {
  beforeEach(() => invoke.mockClear());

  it('a text-only send is the same call as before: no attachments field', () => {
    void api.agentMessage('hello');
    expect(invoke).toHaveBeenCalledWith('agent_message', { sessionId: 'main', text: 'hello' });
    const args = invoke.mock.calls[0][1] ?? {};
    expect(args).not.toHaveProperty('attachments');
  });

  it('an empty attachments list leaves the field out too', () => {
    void api.agentMessage('hello', []);
    expect(invoke.mock.calls[0][1]).toEqual({ sessionId: 'main', text: 'hello' });
  });

  it('attachments are sent as given, with the text (which may be empty)', () => {
    void api.agentMessage('', [IMAGE]);
    expect(invoke).toHaveBeenCalledWith('agent_message', { sessionId: 'main', text: '', attachments: [IMAGE] });
  });
});
