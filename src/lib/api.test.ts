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

  it('suggestPrompts takes no arguments and asks for the suggestions', () => {
    void api.suggestPrompts();
    expect(invoke).toHaveBeenCalledWith('suggest_prompts', { sessionId: 'main' });
  });

  it('focusMainWindow takes no arguments and asks the backend to bring the window forward', () => {
    void api.focusMainWindow();
    expect(invoke).toHaveBeenCalledWith('focus_main_window', undefined);
  });

  it('openDevtools takes no arguments and asks the backend to open DevTools', () => {
    void api.openDevtools();
    expect(invoke).toHaveBeenCalledWith('open_devtools', undefined);
  });

  it('compactConversation takes no arguments', () => {
    void api.compactConversation();
    expect(invoke).toHaveBeenCalledWith('compact_conversation', { sessionId: 'main' });
  });

  it('forkConversation takes no arguments', () => {
    void api.forkConversation();
    expect(invoke).toHaveBeenCalledWith('fork_conversation', { sessionId: 'main' });
  });

  it('exportConversation asks for the live conversation as Markdown', () => {
    void api.exportConversation();
    expect(invoke).toHaveBeenCalledWith('export_conversation', { sessionId: 'main' });
  });
});

describe('api MCP commands', () => {
  beforeEach(() => invoke.mockClear());

  const SERVER = {
    config: { name: 'fetch', enabled: true, approval: 'ask' as const, transport: { type: 'stdio' as const, command: 'uvx', args: [], env: ['TOKEN'] } },
    env_values: { TOKEN: 'typed-value' },
    header_values: {},
  };

  it('getMcp takes no arguments', () => {
    void api.getMcp();
    expect(invoke).toHaveBeenCalledWith('get_mcp', undefined);
  });

  it('mcpImport sends the pasted text as text', () => {
    void api.mcpImport('{"mcpServers": {}}');
    expect(invoke).toHaveBeenCalledWith('mcp_import', { text: '{"mcpServers": {}}' });
  });

  it('mcpSaveServer sends the input as input, with its secret values', () => {
    void api.mcpSaveServer(SERVER);
    expect(invoke).toHaveBeenCalledWith('mcp_save_server', { input: SERVER });
  });

  it('mcpSetToolApproval sends a null level as null, which clears the override', () => {
    void api.mcpSetToolApproval('fetch', 'search', null);
    expect(invoke).toHaveBeenCalledWith('mcp_set_tool_approval', { name: 'fetch', tool: 'search', level: null });
    void api.mcpSetToolApproval('fetch', 'search', 'never');
    expect(invoke).toHaveBeenLastCalledWith('mcp_set_tool_approval', { name: 'fetch', tool: 'search', level: 'never' });
  });

  it('the server commands send the server name', () => {
    void api.mcpRemoveServer('fetch');
    expect(invoke).toHaveBeenLastCalledWith('mcp_remove_server', { name: 'fetch' });
    void api.mcpSetEnabled('fetch', false);
    expect(invoke).toHaveBeenLastCalledWith('mcp_set_enabled', { name: 'fetch', enabled: false });
    void api.mcpSetServerApproval('fetch', 'auto');
    expect(invoke).toHaveBeenLastCalledWith('mcp_set_server_approval', { name: 'fetch', level: 'auto' });
    void api.mcpRestart('fetch');
    expect(invoke).toHaveBeenLastCalledWith('mcp_restart', { name: 'fetch' });
    void api.mcpLogs('fetch');
    expect(invoke).toHaveBeenLastCalledWith('mcp_logs', { name: 'fetch' });
  });
});

describe('api skills commands', () => {
  beforeEach(() => invoke.mockClear());

  it('listSkills passes the session id; the backend picks the project folders from the session', () => {
    void api.listSkills();
    expect(invoke).toHaveBeenLastCalledWith('list_skills', { sessionId: 'main' });
  });
});
