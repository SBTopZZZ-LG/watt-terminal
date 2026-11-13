// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs. Every call carries the session id.
// Tauri maps the Rust snake_case argument names to camelCase here (session_id -> sessionId).

import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  ApprovalPreview,
  ApprovalSettings,
  AttachmentInput,
  AutoApproveState,
  CommandCatalog,
  ConversationMeta,
  DeleteResult,
  FetchOutcome,
  IntegrateOutcome,
  IntegrationSettings,
  LimitsSettings,
  LoadedConversation,
  ModelRef,
  ProviderPreset,
  QuestionAnswerInput,
  RecoverAction,
  SaveProviderInput,
  SettingsView,
  StartResult,
  Status,
  TestResult,
  UiState,
  UiStatePatch,
  WireEvent,
} from './types';

/** Slice 1 runs one session. Every call still passes its id, so more sessions later need no call-site change. */
export const SESSION_ID = 'main';

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
  startSession(rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('start_session', { sessionId: SESSION_ID, rows, cols, onData, onEvents });
  },
  /** Starts a new shell if the session has exited (new nonce); otherwise attaches like startSession. */
  restartSession(rows: number, cols: number, onData: Channel<ArrayBuffer>, onEvents: Channel<WireEvent>) {
    return invoke<StartResult>('restart_session', { sessionId: SESSION_ID, rows, cols, onData, onEvents });
  },
  ack(bytes: number) {
    return invoke<void>('ack', { sessionId: SESSION_ID, bytes });
  },
  resize(rows: number, cols: number) {
    return invoke<void>('resize', { sessionId: SESSION_ID, rows, cols });
  },
  writeUser(text: string) {
    return invoke<void>('write_user', { sessionId: SESSION_ID, text });
  },
  /**
   * Sends one user message to the agent, with its attachments (images as PNG or JPEG, text documents as UTF-8). The field is
   * left out when there are none, so a text-only send is the same call as before.
   */
  agentMessage(text: string, attachments: AttachmentInput[] = []) {
    return invoke<void>('agent_message', attachments.length > 0 ? { sessionId: SESSION_ID, text, attachments } : { sessionId: SESSION_ID, text });
  },
  /** Cancels the running agent turn, if any. Harmless when none is running. */
  cancelAgent() {
    return invoke<void>('cancel_agent', { sessionId: SESSION_ID });
  },
  /** Clears the agent's conversation. Refused while the agent is busy. */
  newConversation() {
    return invoke<void>('new_conversation', { sessionId: SESSION_ID });
  },
  approveTool(callId: string, approve: boolean) {
    return invoke<void>('approve_tool', { sessionId: SESSION_ID, callId, approve });
  },
  setAutoApprove(on: boolean) {
    return invoke<void>('set_auto_approve', { sessionId: SESSION_ID, on });
  },
  /**
   * Answers the agent's request to integrate the shell (its `asking` event). `accept` runs the integration; anything else
   * declines it. Errors when no such request is pending (it timed out, or the turn ended).
   */
  answerIntegration(callId: string, accept: boolean) {
    return invoke<void>('answer_integration', { sessionId: SESSION_ID, callId, accept });
  },
  /**
   * Answers the agent's clarification question (its `question_request` event). Errors when the answers are invalid
   * (the card stays and can retry) or no such question is pending (it already settled).
   */
  answerQuestions(callId: string, answers: QuestionAnswerInput[]) {
    return invoke<void>('answer_questions', { sessionId: SESSION_ID, callId, answers });
  },
  /** Approve-all as the shell on top sees it. Read again after a levels event or a toggle. */
  autoApproveState() {
    return invoke<AutoApproveState>('auto_approve_state', { sessionId: SESSION_ID });
  },
  /**
   * Integrates the shell in the foreground (ssh, a nested shell, su, a container exec) as a new level. Slow: it waits for the
   * remote shell. While it runs, the terminal and the input bar take no keys.
   */
  integrateShell() {
    return invoke<IntegrateOutcome>('integrate_shell', { sessionId: SESSION_ID });
  },
  getStatus() {
    return invoke<Status>('get_status', { sessionId: SESSION_ID });
  },
  /** The custom slash-command templates (global and project dirs), plus the two dirs so `/help` can show them. */
  listCommands() {
    return invoke<CommandCatalog>('list_commands', { sessionId: SESSION_ID });
  },
  /**
   * Asks a one-off side question about the live conversation. One model call outside a turn; the answer is not
   * stored and the transcript only gets a notice. Refused while the agent is busy.
   */
  askBtw(text: string) {
    return invoke<string>('ask_btw', { sessionId: SESSION_ID, text });
  },
  /**
   * Suggests a title for the live conversation. One model call outside a turn; nothing is stored and the
   * transcript only gets notices. Refused while the agent is busy, and when there is nothing to name yet.
   */
  suggestName() {
    return invoke<string>('suggest_name', { sessionId: SESSION_ID });
  },
  /** Summarizes the live conversation into a new conversation and switches to it. The old one stays in history. */
  compactConversation() {
    return invoke<LoadedConversation>('compact_conversation', { sessionId: SESSION_ID });
  },
  /** Copies the live conversation into a new one and switches to it. The old one stays untouched. */
  forkConversation() {
    return invoke<LoadedConversation>('fork_conversation', { sessionId: SESSION_ID });
  },
  /** The saved conversations, most recently updated first. */
  listConversations() {
    return invoke<ConversationMeta[]>('list_conversations');
  },
  /** Opens a saved conversation as the live one. Refused while the agent is busy. */
  loadConversation(id: string) {
    return invoke<LoadedConversation>('load_conversation', { sessionId: SESSION_ID, id });
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
  /** Saves the per-turn caps. Refused while settings.json is broken. A value out of range fails with its name in the message. */
  setLimitsSettings(input: LimitsSettings) {
    return invoke<SettingsView>('set_limits_settings', { input });
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
};
