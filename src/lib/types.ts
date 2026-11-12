// Wire types. These mirror the serde shapes in src-tauri/src/core (observer.rs, record.rs, agent.rs, lib.rs).
// Field names are snake_case because that is what Rust sends.

export type InputState =
  | 'booting'
  | 'prompt'
  | 'submitted'
  | 'running'
  | 'continuation'
  | 'unresponsive'
  | 'alt'
  | 'raw'
  | 'exited';

export type IntegrationStatus = 'booting' | 'live' | 'raw';
export type ObserverStatus = 'ok' | 'desynced';

export interface Cwd {
  host: string;
  path: string;
}

/**
 * What the command line says is running in the foreground (core/foreground.rs). A hint, guessed from the words: `other` is
 * an ordinary program. The others (ssh, a nested shell, su, a container exec, a multiplexer) have no shell integration, so
 * the agent cannot run commands in them.
 */
export type ForegroundKind = 'ssh' | 'shell' | 'su' | 'container_exec' | 'multiplexer' | 'other';

export interface Foreground {
  kind: ForegroundKind;
  command: string;
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

/** Why a nested shell could not be integrated (core/observer.rs InjectRefusal). */
export type InjectRefusal = 'not_running' | 'alt_screen' | 'no_shell_program' | 'too_deep' | 'busy';

/** What `integrate_shell` reports (core/session.rs IntegrateOutcome). */
export type IntegrateOutcome =
  | { outcome: 'integrated'; key: number; label: string; sh: string; user: string | null; host: string | null }
  | { outcome: 'unsupported_shell' }
  | { outcome: 'timeout' }
  | { outcome: 'failed'; hint: string }
  | { outcome: 'refused'; reason: InjectRefusal };

/** Approve-all as the shell on top sees it (`auto_approve_state`). `suspended` means it is on in another shell level. */
export interface AutoApproveState {
  auto: boolean;
  suspended: boolean;
}

/** One shell level (core/observer.rs). `key` 0 is the local shell. Nested levels are integrated shells inside another program. */
export interface LevelInfo {
  key: number;
  kind: 'local' | 'remote';
  label: string;
  sh: string | null;
  host: string | null;
  user: string | null;
  state: InputState;
}

export type SessionEvent =
  /** `foreground` is left out when there is no hint: not Running, or a blank command. An ordinary command is kind `other`. */
  | { type: 'state'; state: InputState; foreground?: Foreground }
  | { type: 'integration'; status: IntegrationStatus }
  | { type: 'observer'; status: ObserverStatus }
  | { type: 'cwd'; cwd: Cwd | null }
  | { type: 'record_started'; id: number; command: string; source: Source }
  | { type: 'record_finished'; record: RecordSummary }
  /** The shell levels changed. `nonces` are every accepted nonce, for checking OSC events. */
  | { type: 'levels'; levels: LevelInfo[]; nonces: string[] };

/**
 * How a tool call was allowed or stopped. read: a read tool, no approval. manual: the user approved. auto: auto-approve
 * skipped the card. denied: not approved (denied, cancelled or timed out). refused: refused before it ran, without asking.
 */
export type ToolDecision = 'read' | 'manual' | 'auto' | 'denied' | 'refused';

/**
 * Why a call asked or ran without asking (core/approval). Wire codes, as sent in `reason`. "approve_all", "rule", "interrupt"
 * and "latched" are the reasons a call ran without asking; the others say why it asked.
 */
export type ApprovalReasonCode =
  | 'approve_all'
  | 'rule'
  | 'interrupt'
  | 'latched'
  | 'manual_mode'
  | 'send_keys'
  | 'unintegrated_shell'
  | 'gate_chars'
  | 'gate_path'
  | 'gate_denylist'
  | 'deny_rule'
  | 'no_rule'
  | 'auto_cap'
  | 'nested_level'
  | 'approve_all_suspended';

export type AgentEvent =
  | { type: 'text'; text: string }
  /** Streamed assistant text. `text` events still carry whole messages. */
  | { type: 'text_delta'; text: string }
  /** The model is working on a reply (true) or has produced its first output (false). */
  | { type: 'thinking'; active: boolean }
  | { type: 'tool_call'; call_id: string; name: string; args: unknown }
  /** `reason` is absent for read tools, `todo_write` and calls refused before the policy. */
  | { type: 'tool_result'; call_id: string; name: string; ok: boolean; decision: ToolDecision; reason?: ApprovalReasonCode; result: unknown }
  /**
   * `reason` is why the call asks. `level` (0 is the local shell) and `location` (a nested shell's label) are always sent by the
   * backend, optional here for existing fixtures. `display` is the decoded input of a send_keys card (`echo hi⏎`), sent only for that.
   */
  | { type: 'approval_request'; call_id: string; name: string; text: string; display?: string; reason?: ApprovalReasonCode; level?: number; location?: string }
  /** The agent is executing an approved shell call (true) and has finished it (false). */
  | { type: 'agent_control'; active: boolean }
  /**
   * A request to integrate the shell (the request_integration tool). `asking`: the user is asked. `running`: the handshake
   * runs (`label` names the shell). `done`: it ended with `outcome`, or with `declined` when the user did not agree.
   */
  | { type: 'integration_request'; call_id: string; phase: 'asking' | 'running' | 'done'; label?: string; outcome?: AgentIntegrationOutcome }
  /** The conversation's todo list was replaced by a successful todo_write. `items` is the whole list. */
  | { type: 'todo_update'; items: TodoItem[] }
  /** The context usage of a model request: sent before the request, and again with the provider's count once a reply reports it. */
  | ({ type: 'context_usage' } & ContextUsage)
  | { type: 'done' };

/** The context usage of one model request (core/chat/context). `used` is the estimate, or the provider's count once `calibrated`. */
export interface ContextUsage {
  /** The model's context limit in tokens, or null when it is not known. */
  limit: number | null;
  used: number;
  reserved_output: number;
  /** True when older content was left out of the request. Always false for now. */
  trimmed: boolean;
  calibrated: boolean;
  categories: { id: string; tokens: number }[];
}

/** What an agent's integration request ended with: the handshake's outcome, or `declined` (no integration, no handshake). */
export type AgentIntegrationOutcome = IntegrateOutcome | { outcome: 'declined' };

export type WireEvent = SessionEvent | AgentEvent;

export interface StartResult {
  spawned: boolean;
  nonce: string;
}

export interface SessionStats {
  forged_events: number;
  /** Tagged events from popped shell levels. Optional so an older status payload still type-checks. */
  stale_events?: number;
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
  /** The foreground hint while a command runs. Left out otherwise. */
  foreground?: Foreground;
  /** The nested shell levels, outermost first. The local shell is not listed. */
  levels: LevelInfo[];
  /** How many shell levels there are, the local one included. 1 means no nested shell. */
  depth: number;
  /** Every accepted OSC nonce (the local shell's and the live levels'). Checked against the events. */
  nonces: string[];
  integration: IntegrationStatus;
  observer: ObserverStatus;
  cwd: Cwd | null;
  stats: SessionStats;
  auto_approve: boolean;
  /** Approve-all is on but was switched on in another shell level, so it does not apply to the shell on top. */
  auto_approve_suspended: boolean;
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
  /** The context usage the conversation ended with, or null when none was saved (see ContextUsage). */
  context: ContextUsage | null;
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
  /** Whether the provider reported image input. Absent or null when unknown. */
  vision?: boolean | null;
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

/** `manual`: every run_command waits for the user. `regex`: the hard gate, then the deny and allow rules, decide. */
export type ApprovalMode = 'manual' | 'regex';

/**
 * How a nested shell (ssh, a shell the app integrated) is approved. `ask`: its commands always need a card, and approve-all
 * applies only in the shell it was switched on in. `same_as_local`: the rules and approve-all apply there as they do locally.
 * The hard gate and the deny rules apply in both.
 */
export type Nested = 'ask' | 'same_as_local';

/** How run_command is approved, as saved. Approve-all is session-only and is not here. */
export interface ApprovalSettings {
  mode: ApprovalMode;
  /** One regular expression per entry. A command runs without asking when one matches the whole command. */
  allow: string[];
  /** Matched the same way. A deny match wins over an allow match. */
  deny: string[];
  nested: Nested;
}

/** `ask`: the user confirms each request_integration. `auto`: the shell is integrated without a dialog. */
export type AgentRequests = 'ask' | 'auto';

/** The shell integration settings (core/settings IntegrationSettings). Saved by their own command. */
export interface IntegrationSettings {
  agent_requests: AgentRequests;
}

/**
 * The per-turn caps of the agent (core/settings LimitsSettings). Saved by set_limits_settings. `max_model_calls` is a u32 with
 * no upper bound in the UI; `model_calls_unlimited` turns the cap off and keeps the number. The polling cap counts only the
 * polling replies (wait, get_screen, send_keys while a command the agent started is running).
 */
export interface LimitsSettings {
  max_model_calls: number;
  model_calls_unlimited: boolean;
  max_poll_calls: number;
  max_commands_per_turn: number;
  max_wait_minutes: number;
}

/** What the saved approval settings would decide for one command, without running it (test_approval). */
export interface ApprovalPreview {
  verdict: 'auto' | 'ask';
  reason: ApprovalReasonCode;
}

export interface Settings {
  version: number;
  providers: Provider[];
  default_model: ModelRef | null;
  /** Tokens. null means no cap. */
  global_context_limit: number | null;
  approval: ApprovalSettings;
  integration: IntegrationSettings;
  limits: LimitsSettings;
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

/**
 * One attachment as agent_message takes it (src-tauri/src/core/chat/attachments.rs, AttachmentInput). An image is PNG or JPEG,
 * already re-encoded in the webview. A text document is its UTF-8 bytes. `thumb` is a PNG or JPEG data URL of at most 16 KB.
 */
export interface AttachmentInput {
  name: string;
  mime: string;
  kind: 'image' | 'text';
  data_b64: string;
  thumb?: string;
  w?: number;
  h?: number;
}
