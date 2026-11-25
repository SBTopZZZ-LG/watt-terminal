// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs. Tab (terminal) commands carry `sessionId`, chat
// (agent) commands carry `chatId`. Tauri maps the Rust snake_case argument names to camelCase here.

import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AgentSettings,
  ApprovalPreview,
  ApprovalSettings,
  AttachmentInput,
  AutoApproveState,
  ChatInfo,
  CommandCatalog,
  ConversationMeta,
  DeleteResult,
  FetchOutcome,
  IntegrateOutcome,
  IntegrationSettings,
  LimitsSettings,
  LoadedConversation,
  McpImportResult,
  McpLevel,
  McpServerInput,
  McpStatus,
  McpView,
  ModelRef,
  ProviderPreset,
  QuestionAnswerInput,
  QueueMode,
  RecoverAction,
  SaveProviderInput,
  SettingsView,
  SkillsSettingsInput,
  SkillsView,
  StartResult,
  Status,
  TabInfo,
  TestResult,
  UiState,
  UiStatePatch,
  WireEvent,
} from './types';

/** The first terminal's id. Tab commands pass it as `sessionId`. */
export const SESSION_ID = 'main';

/** The default chat's id. Chat commands pass it as `chatId`. */
export const CHAT_ID = 'main';

export function errorText(e: unknown): string {
  if (typeof e === 'string') {
    return e;
  }
  if (e instanceof Error) {
    return e.message;
  }
  return String(e);
}

export const api = {
  startSession(sessionId: string, rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('start_session', { sessionId, rows, cols, onData, onEvents });
  },
  /** Starts a new shell if the session has exited (new nonce); otherwise attaches like startSession. */
  restartSession(sessionId: string, rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('restart_session', { sessionId, rows, cols, onData, onEvents });
  },
  /** Removes a terminal and kills its shell, at any time. An agent turn that held it is not cancelled. */
  closeSession(sessionId: string) {
    return invoke<void>('close_session', { sessionId });
  },
  /** Sets a terminal's title. Null or blank clears it. The backend trims and caps it. */
  setTabTitle(sessionId: string, title: string | null) {
    return invoke<void>('set_tab_title', { sessionId, title });
  },
  /** Every terminal, oldest first. */
  listTabs() {
    return invoke<TabInfo[]>('list_tabs');
  },
  /** Assigns a terminal to a chat (`on`), or unassigns it. Refused when another chat owns it, or when the chat is busy and would lose it. */
  assignTab(chatId: string, sessionId: string, on: boolean) {
    return invoke<void>('assign_tab_cmd', { chatId, sessionId, on });
  },
  /** Creates a chat with no terminal and a new conversation. Returns its id. */
  createChat() {
    return invoke<string>('create_chat');
  },
  /** Sets the chat's agent-events channel. Idempotent: a later call replaces the channel. */
  attachChat(chatId: string, onEvents: Channel<WireEvent>) {
    return invoke<void>('attach_chat', { chatId, onEvents });
  },
  /** Closes a chat (refused while its turn runs) and unassigns its terminals. */
  closeChat(chatId: string) {
    return invoke<void>('close_chat', { chatId });
  },
  /** Every chat, by id. */
  listChats() {
    return invoke<ChatInfo[]>('list_chats');
  },
  ack(sessionId: string, bytes: number) {
    return invoke<void>('ack', { sessionId, bytes });
  },
  resize(sessionId: string, rows: number, cols: number) {
    return invoke<void>('resize', { sessionId, rows, cols });
  },
  writeUser(sessionId: string, text: string) {
    return invoke<void>('write_user', { sessionId, text });
  },
  /**
   * Sends one user message to the agent, with its attachments (images as PNG or JPEG, text documents as UTF-8). The field is
   * left out when there are none, so a text-only send is the same call as before.
   */
  agentMessage(text: string, attachments: AttachmentInput[] = []) {
    // `sha256` is for the transcript only (see AttachmentInput), so it is not sent.
    const wire = attachments.map(({ sha256: _sha256, ...rest }) => rest);
    return invoke<void>('agent_message', wire.length > 0 ? { chatId: CHAT_ID, text, attachments: wire } : { chatId: CHAT_ID, text });
  },
  /**
   * Steers a message into the running agent turn: it goes in at the turn's next step. Resolves true when accepted, false when
   * not (no agent turn running, the turn ending, or a side command running): the caller then queues it as before. `mode` is
   * the queue mode: `all` sends every waiting message together, `one` the first alone.
   */
  steerMessage(id: number, text: string, attachments: AttachmentInput[] = [], mode: QueueMode = 'all') {
    // As for agentMessage, `sha256` is for the transcript only, so it is not sent.
    const wire = attachments.map(({ sha256: _sha256, ...rest }) => rest);
    const args = wire.length > 0 ? { chatId: CHAT_ID, id, text, attachments: wire, mode } : { chatId: CHAT_ID, id, text, mode };
    return invoke<boolean>('steer_message', args);
  },
  /** Takes a steered message back. True when it was still waiting, false when the turn had already taken it. */
  withdrawSteer(id: number) {
    return invoke<boolean>('withdraw_steer', { chatId: CHAT_ID, id });
  },
  /** Cancels the running agent turn, if any. Harmless when none is running. */
  cancelAgent() {
    return invoke<void>('cancel_agent', { chatId: CHAT_ID });
  },
  /** Clears the agent's conversation. Refused while the agent is busy. */
  newConversation() {
    return invoke<void>('new_conversation', { chatId: CHAT_ID });
  },
  approveTool(callId: string, approve: boolean) {
    return invoke<void>('approve_tool', { chatId: CHAT_ID, callId, approve });
  },
  setAutoApprove(on: boolean) {
    return invoke<void>('set_auto_approve', { chatId: CHAT_ID, on });
  },
  /**
   * Answers the agent's request to integrate the shell (its `asking` event). `accept` runs the integration; anything else
   * declines it. Errors when no such request is pending (it timed out, or the turn ended).
   */
  answerIntegration(callId: string, accept: boolean) {
    return invoke<void>('answer_integration', { chatId: CHAT_ID, callId, accept });
  },
  /**
   * Answers the agent's clarification question (its `question_request` event). Errors when the answers are invalid
   * (the card stays and can retry) or no such question is pending (it already settled).
   */
  answerQuestions(callId: string, answers: QuestionAnswerInput[]) {
    return invoke<void>('answer_questions', { chatId: CHAT_ID, callId, answers });
  },
  /** Approve-all as the shell on top sees it. Read again after a levels event or a toggle. */
  autoApproveState() {
    return invoke<AutoApproveState>('auto_approve_state', { chatId: CHAT_ID });
  },
  /**
   * Integrates the shell in the foreground (ssh, a nested shell, su, a container exec) as a new level. Slow: it waits for the
   * remote shell. While it runs, the terminal and the input bar take no keys.
   */
  integrateShell(sessionId: string) {
    return invoke<IntegrateOutcome>('integrate_shell', { sessionId });
  },
  getStatus() {
    return invoke<Status>('get_status', { chatId: CHAT_ID });
  },
  /** The custom slash-command templates (global and project dirs), plus the two dirs so `/help` can show them. */
  listCommands(sessionId: string) {
    return invoke<CommandCatalog>('list_commands', { sessionId });
  },
  /**
   * Asks a one-off side question about the live conversation. One model call outside a turn; the answer is not
   * stored and the transcript only gets a notice. Refused while the agent is busy.
   */
  askBtw(text: string) {
    return invoke<string>('ask_btw', { chatId: CHAT_ID, text });
  },
  /**
   * Suggests a title for the live conversation. One model call outside a turn; nothing is stored and the
   * transcript only gets notices. Refused while the agent is busy, and when there is nothing to name yet.
   */
  suggestName() {
    return invoke<string>('suggest_name', { chatId: CHAT_ID });
  },
  /**
   * Suggests a few follow-up prompts after the latest reply. One read-only model call: it never blocks a turn or a send,
   * and nothing is stored. Empty when there is nothing to suggest. Errors reject; the caller ignores them.
   */
  suggestPrompts() {
    return invoke<string[]>('suggest_prompts', { chatId: CHAT_ID });
  },
  /** Brings the main window forward, for a clicked notification. Errors reject; the caller ignores them. */
  focusMainWindow() {
    return invoke<void>('focus_main_window');
  },
  /** Opens DevTools for the main window (the layout inspector). Refused by the backend unless the developer setting is on. Errors reject. */
  openDevtools() {
    return invoke<void>('open_devtools');
  },
  /** Summarizes the live conversation into a new conversation and switches to it. The old one stays in history. */
  compactConversation() {
    return invoke<LoadedConversation>('compact_conversation', { chatId: CHAT_ID });
  },
  /** Copies the live conversation into a new one and switches to it. The old one stays untouched. */
  forkConversation() {
    return invoke<LoadedConversation>('fork_conversation', { chatId: CHAT_ID });
  },
  /** The live conversation as Markdown for `/export`: the stored history, so nothing is lost to the UI cap. */
  exportConversation() {
    return invoke<string>('export_conversation', { chatId: CHAT_ID });
  },
  /** The saved conversations, most recently updated first. */
  listConversations() {
    return invoke<ConversationMeta[]>('list_conversations');
  },
  /** Opens a saved conversation as the live one. Refused while the agent is busy. */
  loadConversation(id: string) {
    return invoke<LoadedConversation>('load_conversation', { chatId: CHAT_ID, id });
  },
  /** The full image of an attachment of the open conversation, as a PNG or JPEG data URL. Rejects when it is no longer stored. */
  getAttachment(sha256: string, mime: string) {
    return invoke<string>('get_attachment', { chatId: CHAT_ID, sha256, mime });
  },
  /** Deletes a saved conversation. Refused while the agent is busy. `deleted_live` says the live conversation was reset. */
  deleteConversation(id: string) {
    return invoke<DeleteResult>('delete_conversation', { id });
  },
  renameConversation(id: string, title: string) {
    return invoke<ConversationMeta>('rename_conversation', { id, title });
  },
  /** Deletes every saved conversation. Refused while the agent is busy. The live conversation is reset. */
  deleteAllConversations() {
    return invoke<DeleteResult>('delete_all_conversations');
  },
  getUiState() {
    return invoke<UiState>('get_ui_state');
  },
  /** Applies a patch to the UI state and returns the state as stored. */
  patchUiState(patch: UiStatePatch) {
    return invoke<UiState>('patch_ui_state', { patch });
  },
  /** The AI provider settings, with which secrets are stored (never their values). */
  getSettings() {
    return invoke<SettingsView>('get_settings');
  },
  /**
   * Recovers from a broken settings.json. "revert" keeps the broken file aside and restores the last working copy;
   * "use_last_good" runs the last working copy for this session and leaves settings.json as it is.
   */
  recoverSettings(action: RecoverAction) {
    return invoke<SettingsView>('recover_settings', { action });
  },
  /** Re-reads settings.json. A file that is valid again makes the AI settings usable again. */
  reloadSettings() {
    return invoke<SettingsView>('reload_settings');
  },
  /** Adds or replaces a provider. Secrets in the input are stored, not returned. */
  saveProvider(input: SaveProviderInput) {
    return invoke<SettingsView>('save_provider', { input });
  },
  deleteProvider(providerId: string) {
    return invoke<SettingsView>('delete_provider', { id: providerId });
  },
  /** Sets the default model, or clears it with null. */
  setDefaultModel(model: ModelRef | null) {
    return invoke<SettingsView>('set_default_model', { model });
  },
  /** Sets the global context cap in tokens, or clears it with null. */
  setGlobalContextLimit(limit: number | null) {
    return invoke<SettingsView>('set_global_context_limit', { limit });
  },
  providerPresets() {
    return invoke<ProviderPreset[]>('provider_presets');
  },
  /** Saves the approval mode and rules. Refused while settings.json is broken. A bad rule fails with its number named. */
  setApprovalSettings(input: ApprovalSettings) {
    return invoke<SettingsView>('set_approval_settings', { input });
  },
  /** Saves whether the agent's integration requests ask the user or run at once. Refused while settings.json is broken. */
  setIntegrationSettings(input: IntegrationSettings) {
    return invoke<SettingsView>('set_integration_settings', { input });
  },
  /** Saves what a denied tool call does: the agent carries on, or the turn stops. Refused while settings.json is broken. */
  setAgentSettings(input: AgentSettings) {
    return invoke<SettingsView>('set_agent_settings', { input });
  },
  /** Saves the per-turn caps. Refused while settings.json is broken. A value out of range fails with its name in the message. */
  setLimitsSettings(input: LimitsSettings) {
    return invoke<SettingsView>('set_limits_settings', { input });
  },
  /** Saves the skill folders (null: the defaults), the switched-off skills and the approval levels. Refused while settings.json is broken, or when a folder is refused. */
  setSkillsSettings(input: SkillsSettingsInput) {
    return invoke<SettingsView>('set_skills_settings', { input });
  },
  /** What the saved approval settings would decide for a command, without running it. Approve-all is not part of it. */
  testApproval(command: string) {
    return invoke<ApprovalPreview>('test_approval', { command });
  },
  /** The built-in deny list: first words that always ask, whatever the rules say. */
  approvalBuiltinDeny() {
    return invoke<string[]>('approval_builtin_deny');
  },
  /** Fetches the provider's model list (one request to the provider) and merges it into the saved models. */
  fetchModels(providerId: string) {
    return invoke<FetchOutcome>('fetch_models', { providerId });
  },
  /** Sends one short test message (a few tokens) to a model. The model need not be the default. */
  testConnection(providerId: string, modelId: string) {
    return invoke<TestResult>('test_connection', { providerId, modelId });
  },
  /** The MCP servers as saved, with their live state and tools. Starts the servers on first use. */
  getMcp() {
    return invoke<McpView>('get_mcp');
  },
  /** The composer's MCP view: names, switches, states and tool counts. Never starts the servers, so it is cheap to poll. */
  getMcpStatus() {
    return invoke<McpStatus>('get_mcp_status');
  },
  /** Adds the servers of a pasted `mcpServers` list. Refused as a whole when a name already exists. */
  mcpImport(text: string) {
    return invoke<McpImportResult>('mcp_import', { text });
  },
  /** Adds a server, or replaces the one with its name. Values in the input are stored, not returned. */
  mcpSaveServer(input: McpServerInput) {
    return invoke<McpView>('mcp_save_server', { input });
  },
  mcpRemoveServer(name: string) {
    return invoke<McpView>('mcp_remove_server', { name });
  },
  mcpSetEnabled(name: string, enabled: boolean) {
    return invoke<McpView>('mcp_set_enabled', { name, enabled });
  },
  mcpSetServerApproval(name: string, level: McpLevel) {
    return invoke<McpView>('mcp_set_server_approval', { name, level });
  },
  /** Sets one tool's level, or clears its override with null (the tool then inherits the server's level). */
  mcpSetToolApproval(name: string, tool: string, level: McpLevel | null) {
    return invoke<McpView>('mcp_set_tool_approval', { name, tool, level });
  },
  /** The skills in the user's folders and, when the session's local shell is on top, its project's. Missing folders are fine. */
  listSkills() {
    return invoke<SkillsView>('list_skills', { chatId: CHAT_ID });
  },
  /** Opens the native "choose a folder" dialog. Resolves with the chosen folder as a full path, or null when the user cancels. Rejects when the dialog cannot open. */
  async pickFolder(defaultPath?: string): Promise<string | null> {
    // Loaded on first use, so code that runs outside Tauri (tests, the Vite preview) does not need the plugin.
    const { open } = await import('@tauri-apps/plugin-dialog');
    return open({ directory: true, multiple: false, title: 'Choose a skill folder', defaultPath });
  },
  mcpRestart(name: string) {
    return invoke<McpView>('mcp_restart', { name });
  },
  mcpLogs(name: string) {
    return invoke<string[]>('mcp_logs', { name });
  },
};
