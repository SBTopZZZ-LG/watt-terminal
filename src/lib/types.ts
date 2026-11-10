// Wire types. These mirror the serde shapes in src-tauri/src/core (observer.rs, record.rs, agent.rs, lib.rs).
// Field names are snake_case because that is what Rust sends.

export type InputState =
  | 'booting'
  | 'prompt'
  | 'submitted'
  | 'running'
  | 'continuation'
  | 'alt'
  | 'raw'
  | 'exited';

export type IntegrationStatus = 'booting' | 'live' | 'raw';
export type ObserverStatus = 'ok' | 'desynced';

export interface Cwd {
  host: string;
  path: string;
}

export type Source =
  | { kind: 'user' }
  | { kind: 'agent'; call_id: string }
  | { kind: 'term_reply' }
  | { kind: 'unknown' };

export type RecordStatus = 'complete' | 'incomplete' | 'approximate';

export interface RecordSummary {
  id: number;
  command: string;
  source: Source;
  exit_code: number | null;
  status: RecordStatus;
  used_alt_screen: boolean;
  output_bytes: number;
}

export type SessionEvent =
  | { type: 'state'; state: InputState }
  | { type: 'integration'; status: IntegrationStatus }
  | { type: 'observer'; status: ObserverStatus }
  | { type: 'cwd'; cwd: Cwd | null }
  | { type: 'record_started'; id: number; command: string; source: Source }
  | { type: 'record_finished'; record: RecordSummary };

/**
 * How a tool call was allowed or stopped. read: a read tool, no approval. manual: the user approved. auto: auto-approve
 * skipped the card. denied: not approved (denied, cancelled or timed out). refused: refused before it ran, without asking.
 */
export type ToolDecision = 'read' | 'manual' | 'auto' | 'denied' | 'refused';

export type AgentEvent =
  | { type: 'text'; text: string }
  /** Streamed assistant text. `text` events still carry whole messages. */
  | { type: 'text_delta'; text: string }
  /** The model is working on a reply (true) or has produced its first output (false). */
  | { type: 'thinking'; active: boolean }
  | { type: 'tool_call'; call_id: string; name: string; args: unknown }
  | { type: 'tool_result'; call_id: string; name: string; ok: boolean; decision: ToolDecision; result: unknown }
  | { type: 'approval_request'; call_id: string; name: string; text: string }
  /** The agent is executing an approved shell call (true) and has finished it (false). */
  | { type: 'agent_control'; active: boolean }
  /** The conversation's todo list was replaced by a successful todo_write. `items` is the whole list. */
  | { type: 'todo_update'; items: TodoItem[] }
  | { type: 'done' };

export type WireEvent = SessionEvent | AgentEvent;

export interface StartResult {
  spawned: boolean;
  nonce: string;
}

export interface SessionStats {
  forged_events: number;
  overflow_events: number;
  dropped_bytes: number;
}

/**
 * Whether the LLM agent is configured, and its model. `provider` is the saved provider's name, or absent or null when
 * the model comes from the dev environment variables. Never the key or the URL.
 */
export interface LlmStatus {
  enabled: boolean;
  model: string | null;
  provider?: string | null;
}

export interface Status {
  state: InputState;
  integration: IntegrationStatus;
  observer: ObserverStatus;
  cwd: Cwd | null;
  stats: SessionStats;
  auto_approve: boolean;
  agent_busy: boolean;
  agent_control: boolean;
  llm: LlmStatus;
  /** The live conversation's id. */
  conversation_id: string;
  /** The state of settings.json. Null when the app folders could not be found. */
  settings_state: SettingsStateView | null;
}

/** The project a conversation belongs to: fixed when it is first saved. `label` is what the UI shows. */
export interface Workspace {
  host: string;
  root: string;
  label: string;
}

/** A saved conversation's metadata, as the history list shows it. Times are Unix seconds. */
export interface ConversationMeta {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  /** User and assistant messages only. */
  message_count: number;
  workspace: Workspace;
  model: string | null;
}

/** A chat-completions message as stored (role, content, tool_calls, tool_call_id). */
export type ChatMessage = Record<string, unknown>;

/** One item of the conversation's todo list. Position-based: the list is replaced as a whole. */
export type TodoStatus = 'pending' | 'in_progress' | 'completed';

export interface TodoItem {
  content: string;
  status: TodoStatus;
}

/** A saved conversation opened as the live one. `messages` are the repaired messages, as the backend now holds them. */
export interface LoadedConversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  /** The repaired todo list. Absent or empty when the conversation had none. */
  todos?: TodoItem[];
}

/** What a saved-conversation delete reports. `deleted_live` is true when the live conversation was reset to a new one. */
export interface DeleteResult {
  deleted_live: boolean;
}

export type Dock = 'left' | 'top' | 'right' | 'bottom';
export type HistoryGroupBy = 'date' | 'workspace';

export interface PanelSizes {
  left: number | null;
  right: number | null;
  top: number | null;
  bottom: number | null;
}

export interface PanelState {
  dock: Dock;
  sizes: PanelSizes;
  open: boolean;
}

/** The UI state file (`ui-state.json`). Each field is null when it has no value yet. */
export interface UiState {
  version: number;
  panel: PanelState | null;
  font_size: number | null;
  last_conversation_id: string | null;
  history_group_by: HistoryGroupBy | null;
}

/** A change to the UI state. An absent field is left alone, `null` clears it, a value sets it. */
export interface UiStatePatch {
  panel?: PanelState | null;
  font_size?: number | null;
  last_conversation_id?: string | null;
  history_group_by?: HistoryGroupBy | null;
}

// AI provider settings (src-tauri/src/core/settings). Secrets are never in these types: they are only written, as
// `api_key` or `header_secrets` in a SaveProviderInput, and read back as presence flags in SettingsView.

/** A header of a provider. A secret header's `value` is always "" here; its real value is in the secret store. */
export interface HeaderEntry {
  name: string;
  value: string;
  secret: boolean;
  /**
   * Input only (SaveProviderInput): the name this header had when it was saved. A secret header renamed with no typed
   * value keeps its stored value. Never returned, so it is absent from SettingsView.
   */
  previous_name?: string | null;
}

/** `fetched` came from the provider's model list; `custom` was added by hand. */
export type ModelSource = 'fetched' | 'custom';

export interface Model {
  id: string;
  /** The user's override, in tokens. */
  context_limit: number | null;
  /** What the provider reported when the list was fetched, in tokens. */
  provider_context: number | null;
  source: ModelSource;
}

export interface Provider {
  /** Slug: a-z, 0-9, '-' or '_', 1 to 40 characters, starting with a-z or 0-9. */
  id: string;
  name: string;
  /** The full chat-completions URL. */
  chat_url: string;
  /** The full model-list URL, or null when the provider has none. */
  models_url: string | null;
  headers: HeaderEntry[];
  stream: boolean;
  max_tokens: number | null;
  models: Model[];
}

export interface ModelRef {
  provider_id: string;
  model_id: string;
}

export interface Settings {
  version: number;
  providers: Provider[];
  default_model: ModelRef | null;
  /** Tokens. null means no cap. */
  global_context_limit: number | null;
}

/** Which secrets a provider has stored. Never their values. */
export interface SecretsPresent {
  api_key: boolean;
  /** Names of the secret headers that have a stored value. */
  headers: string[];
}

/**
 * `ok`: settings.json is in use. `broken`: it cannot be used, nothing runs on settings and changes are refused.
 * `last_good`: it is broken and this session runs on the last working copy; the file is left as it is.
 */
export type SettingsStateKind = 'ok' | 'broken' | 'last_good';

export interface SettingsStateView {
  kind: SettingsStateKind;
  /** Why the file cannot be used. Null when `kind` is "ok". */
  reason: string | null;
  /** Whether the last working copy exists and checks out, so the recovery actions can run. */
  last_good_available: boolean;
  /** The path of settings.json, for display. */
  path: string;
}

/** The recovery actions for a broken settings.json, as `recover_settings` takes them. */
export type RecoverAction = 'revert' | 'use_last_good';

export interface SettingsView {
  settings: Settings;
  /** Keyed by provider id. */
  secrets: Record<string, SecretsPresent>;
  store_kind: 'keychain' | 'file';
  /** Set while settings.json cannot be used (`settings_state.kind` is not "ok"). Changes are refused until it is fixed. */
  load_error: string | null;
  settings_state: SettingsStateView;
}

/**
 * A provider as saved. `api_key`: absent or null keeps the stored key, "" removes it, anything else replaces it.
 * `header_secrets`: header name to value for secret headers. An absent name keeps the stored value, "" removes it.
 */
export interface SaveProviderInput {
  provider: Provider;
  api_key?: string | null;
  header_secrets?: Record<string, string>;
}

export interface ProviderPreset {
  key: string;
  label: string;
  provider: Provider;
  needs_key: boolean;
  note: string;
}

export interface FetchSummary {
  added: number;
  updated: number;
  removed: number;
  total: number;
}

export interface FetchOutcome {
  view: SettingsView;
  summary: FetchSummary;
}

export interface TestResult {
  ok: boolean;
  message: string;
  millis: number;
}
