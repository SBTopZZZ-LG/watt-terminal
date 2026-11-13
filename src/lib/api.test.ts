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

describe('api.listCommands', () => {
  beforeEach(() => invoke.mockClear());

  it('asks for the command catalog for this session', () => {
    void api.listCommands();
    expect(invoke).toHaveBeenCalledWith('list_commands', { sessionId: 'main' });
  });
});

describe('api.answerQuestions', () => {
  beforeEach(() => invoke.mockClear());

  it('answers the waiting question with its call id and answers', () => {
    const answers = [{ question_id: 'q1', selected: [0], skipped: false }];
    void api.answerQuestions('call-1', answers);
    expect(invoke).toHaveBeenCalledWith('answer_questions', { sessionId: 'main', callId: 'call-1', answers });
  });
});

describe('api.setDefaultModel', () => {
  beforeEach(() => invoke.mockClear());

  it('sends the ref as given, variant included', () => {
    void api.setDefaultModel({ provider_id: 'acme', model_id: 'big/one', variant: 'high' });
    expect(invoke).toHaveBeenCalledWith('set_default_model', {
      model: { provider_id: 'acme', model_id: 'big/one', variant: 'high' },
    });
  });

  it('a base model and a clear pass through unchanged', () => {
    void api.setDefaultModel({ provider_id: 'acme', model_id: 'big/one', variant: null });
    expect(invoke).toHaveBeenCalledWith('set_default_model', {
      model: { provider_id: 'acme', model_id: 'big/one', variant: null },
    });
    void api.setDefaultModel(null);
    expect(invoke).toHaveBeenCalledWith('set_default_model', { model: null });
  });
});

describe('api side commands', () => {
  beforeEach(() => invoke.mockClear());

  it('askBtw sends the question text', () => {
    void api.askBtw('why did this fail?');
    expect(invoke).toHaveBeenCalledWith('ask_btw', { sessionId: 'main', text: 'why did this fail?' });
  });

  it('compactConversation takes no arguments', () => {
    void api.compactConversation();
    expect(invoke).toHaveBeenCalledWith('compact_conversation', { sessionId: 'main' });
  });

  it('forkConversation takes no arguments', () => {
    void api.forkConversation();
    expect(invoke).toHaveBeenCalledWith('fork_conversation', { sessionId: 'main' });
  });
});
