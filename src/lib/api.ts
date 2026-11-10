// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs. Every call carries the session id.
// Tauri maps the Rust snake_case argument names to camelCase here (session_id -> sessionId).

import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  ConversationMeta,
  DeleteResult,
  FetchOutcome,
  LoadedConversation,
  ModelRef,
  ProviderPreset,
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
  agentMessage(text: string) {
    return invoke<void>('agent_message', { sessionId: SESSION_ID, text });
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
  getStatus() {
    return invoke<Status>('get_status', { sessionId: SESSION_ID });
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
  /** Fetches the provider's model list (one request to the provider) and merges it into the saved models. */
  fetchModels(providerId: string) {
    return invoke<FetchOutcome>('fetch_models', { providerId });
  },
  /** Sends one short test message (a few tokens) to a model. The model need not be the default. */
  testConnection(providerId: string, modelId: string) {
    return invoke<TestResult>('test_connection', { providerId, modelId });
  },
};
