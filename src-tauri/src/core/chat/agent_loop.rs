//! The chat agent's turn loop: model calls, tool calls, and the history that carries them.
//!
//! `LlmAgent` implements `Agent`. A turn pushes the user message, then makes model calls until the model answers
//! without tool calls. Each reply's tool calls run one by one through `AgentCtx`, so approvals, events and the shell
//! gate are the same as for the mock. Every result is projected and redacted (core/chat/project) before it enters the
//! history. A denial or a cancel stops the turn: the calls not yet run are closed in the history, and no further
//! model call is made. Every exit path keeps the pairing that `History` checks: no call is left without a result.
//! The caller can ask for a save at each checkpoint (`LlmAgent::with_checkpoint`): after the user message is recorded,
//! once a reply's tool calls are recorded but before they run, and after each completed round of tool calls. The
//! middle checkpoint may hold calls without results yet; loading it repairs them into closed calls.
//!
//! Dev configuration (replaced by a config file later): `WATT_LLM_URL` (full chat-completions URL),
//! `WATT_LLM_KEY` and `WATT_LLM_MODEL` must all be set, or the mock agent is used. Optional:
//! `WATT_LLM_HEADERS` (`Name: value` lines or `;`-separated, where `{session_id}` is the conversation id),
//! `WATT_LLM_STREAM=0` for one non-streamed reply per call, and `WATT_LLM_MAX_TOKENS`. The old `AGENT_TERM_LLM_*`
//! names are still read when the `WATT_LLM_*` one is unset.
//! Nothing here logs the key or a header value.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Value, json};

use crate::core::agent::{Agent, AgentCtx, AgentEvent, DENIED, UserMessage};
use crate::core::env::env_setting;
use crate::core::foreground::{Foreground, ForegroundKind};
use crate::core::chat::attachments::{Resolver, user_content};
use crate::core::chat::context::{self, Calibrator, ContextUsage};
use crate::core::chat::history::History;
use crate::core::chat::instructions;
use crate::core::chat::project::project;
use crate::core::chat::prompt;
use crate::core::chat::questions;
use crate::core::chat::redact::Redactor;
use crate::core::chat::todo::{self, Todo, TodoList};
use crate::core::chat::trim::{self, TrimPlan};
use crate::core::llm::{self, AssistantReply, ChatRequest, Delta, LlmError, ProviderConfig, ToolCall, TurnCancel};
use crate::core::observer::OpenAgentRecord;
use crate::core::persist::attachments::AttachmentStore;
use crate::core::settings::LimitsSettings;
use crate::core::tools::{age_text, foreground_next_step, TopLevel};

const SESSION_PLACEHOLDER: &str = "{session_id}";
pub const USER_AGENT: &str = concat!("watt-terminal/", env!("CARGO_PKG_VERSION"));
const ENV_URL: &str = "WATT_LLM_URL";
const ENV_KEY: &str = "WATT_LLM_KEY";
const ENV_MODEL: &str = "WATT_LLM_MODEL";
const ENV_HEADERS: &str = "WATT_LLM_HEADERS";
const ENV_STREAM: &str = "WATT_LLM_STREAM";
const ENV_MAX_TOKENS: &str = "WATT_LLM_MAX_TOKENS";

/// Tool names the model may call that the loop routes by name. Anything else is an unknown tool. Must match `READ_TOOLS`
/// in core/agent.rs.
const READ_NAMES: [&str; 5] = ["list_commands", "get_command", "get_screen", "get_cwd", "wait"];
/// Longest tool name echoed back in an error, so a model cannot make the history carry a huge name.
const NAME_LIMIT: usize = 80;

/// Tool names that count a reply as polling when they are all it asks for, while a command the agent started is running.
const POLL_NAMES: [&str; 3] = ["wait", "get_screen", "send_keys"];
const LIMIT_REASON: &str = "not run: the turn stopped after the model call limit";
const POLL_LIMIT_REASON: &str = "not run: the turn stopped after the polling call limit";
const DENIED_NOTICE: &str = "Stopped: the command was not approved. Tell me what you want to do next.";
/// Said once, when a model turned out to refuse images and the retry without them worked.
const NO_VISION_NOTICE: &str =
    "This model doesn't accept images, so they were left out of this reply. Switch to a vision model to use them.";

/// The models (by id) that refused images in a conversation. Shared by the conversation's agents, so each turn sees it.
pub type NoVisionModels = Arc<Mutex<HashSet<String>>>;

/// The calibration of a conversation's estimates, shared by its agents (see `Conversation::calibration`).
pub type Calibration = Arc<Mutex<Calibrator>>;

/// The context limits learned per model id from overflow errors, shared by a conversation's agents (see
/// `Conversation::learned_limits`).
pub type LearnedLimits = Arc<Mutex<HashMap<String, u32>>>;

/// The context usage last emitted for a conversation, shared by its agents (see `Conversation::last_usage`).
pub type LastUsage = Arc<Mutex<Option<ContextUsage>>>;

/// The provider settings read from the environment. Holds the key, so `Debug` hides it.
#[derive(Clone)]
pub struct LlmSettings {
    pub chat_url: String,
    pub api_key: String,
    pub model: String,
    /// Header templates as written. `{session_id}` is replaced per conversation by `resolved_headers`.
    pub headers: Vec<(String, String)>,
    /// True streams the reply (text appears as it arrives); false waits for the whole reply.
    pub stream: bool,
    pub max_tokens: Option<u32>,
    /// The context limit that applies to this model, in tokens (core/settings), or None when it is not known. The loop
    /// reports it with the context usage (core/chat/context), and trims the wire copy of a request to fit it
    /// (core/chat/trim). Without a known limit, nothing is trimmed.
    pub context_limit: Option<u32>,
    /// The selected model variant's request overlay (see `ModelVariant::body`), or None for the base model.
    /// Carried into every `ChatRequest` this turn sends (see `LlmAgent::send`).
    pub extra: Option<serde_json::Map<String, Value>>,
}

impl LlmSettings {
    /// The header values with `{session_id}` replaced by the conversation id.
    pub fn resolved_headers(&self, conversation_id: &str) -> Vec<(String, String)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.clone(), value.replace(SESSION_PLACEHOLDER, conversation_id)))
            .collect()
    }

    /// The provider config for one conversation: this conversation's resolved headers and the shared User-Agent.
    pub fn provider_config(&self, conversation_id: &str) -> ProviderConfig {
        ProviderConfig {
            chat_url: self.chat_url.clone(),
            api_key: self.api_key.clone(),
            headers: self.resolved_headers(conversation_id),
            user_agent: USER_AGENT.to_string(),
        }
    }

    /// The redactor for one conversation: the key and this conversation's resolved header values are the secrets the
    /// output must never show.
    pub fn redactor(&self, conversation_id: &str) -> Redactor {
        let mut known = vec![self.api_key.clone()];
        known.extend(self.resolved_headers(conversation_id).into_iter().map(|(_, value)| value));
        Redactor::new(known)
    }
}

impl fmt::Debug for LlmSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        // The overlay's values stay out of the debug text; only its key names are shown.
        let extra_keys: Vec<&str> =
            self.extra.iter().flat_map(|extra| extra.keys().map(String::as_str)).collect();
        f.debug_struct("LlmSettings")
            .field("chat_url", &self.chat_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("header_names", &header_names)
            .field("stream", &self.stream)
            .field("max_tokens", &self.max_tokens)
            .field("context_limit", &self.context_limit)
            .field("extra_keys", &extra_keys)
            .finish()
    }
}

/// Reads the settings from `lookup` (the environment in production). `Ok(None)` when URL, key or model is missing:
/// the mock agent is used. `Err` is a bad value, with a message that never includes a secret.
pub fn settings_from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<LlmSettings>, String> {
    let get = |name: &str| lookup(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let (Some(chat_url), Some(api_key), Some(model)) = (get(ENV_URL), get(ENV_KEY), get(ENV_MODEL)) else {
        return Ok(None);
    };
    if !(chat_url.starts_with("https://") || chat_url.starts_with("http://")) {
        return Err(format!("{ENV_URL} must start with https:// or http://"));
    }
    let headers = parse_headers(&get(ENV_HEADERS).unwrap_or_default())?;
    let stream = match get(ENV_STREAM).as_deref() {
        None | Some("1") => true,
        Some("0") => false,
        Some(_) => return Err(format!("{ENV_STREAM} must be 0 or 1")),
    };
    let max_tokens = match get(ENV_MAX_TOKENS) {
        None => None,
        Some(v) => {
            let n = v.parse::<u32>().ok().filter(|n| *n > 0);
            Some(n.ok_or_else(|| format!("{ENV_MAX_TOKENS} must be a positive whole number"))?)
        }
    };
    Ok(Some(LlmSettings { chat_url, api_key, model, headers, stream, max_tokens, context_limit: None, extra: None }))
}

/// `Name: value` entries, one per line or separated by `;`. Each name and value is checked now, with the placeholder
/// filled by a stand-in, so a bad header fails at startup rather than on the first request.
fn parse_headers(text: &str) -> Result<Vec<(String, String)>, String> {
    let entries = text.split(['\n', ';']).map(str::trim).filter(|e| !e.is_empty());
    let mut out = Vec::new();
    for (index, entry) in entries.enumerate() {
        let number = index + 1;
        let (name, value) = entry
            .split_once(':')
            .map(|(name, value)| (name.trim(), value.trim()))
            .filter(|(name, _)| !name.is_empty())
            .ok_or_else(|| format!("{ENV_HEADERS} entry {number} is not `Name: value`"))?;
        HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("{ENV_HEADERS} entry {number} has an invalid header name"))?;
        HeaderValue::from_str(&value.replace(SESSION_PLACEHOLDER, "ses_0"))
            .map_err(|_| format!("{ENV_HEADERS} entry {number} has an invalid header value"))?;
        out.push((name.to_string(), value.to_string()));
    }
    Ok(out)
}

/// Reads the settings once per process from the environment. A bad value is logged (without the value) and the mock
/// agent is used. `run()` calls this at startup so the warning shows at launch.
pub fn settings() -> Option<&'static LlmSettings> {
    static SETTINGS: OnceLock<Option<LlmSettings>> = OnceLock::new();
    // Each `WATT_LLM_*` name also reads its old `AGENT_TERM_LLM_*` name (core/env.rs); WATT_* wins when both are set.
    SETTINGS
        .get_or_init(|| {
            settings_or_warn(|name| {
                let legacy = name.replacen("WATT_", "AGENT_TERM_", 1);
                env_setting(name, &legacy).and_then(|v| v.into_string().ok())
            })
        })
        .as_ref()
}

/// An owned copy of `settings()`, for callers that keep the settings past the turn (the agent takes ownership).
pub fn settings_cloned() -> Option<LlmSettings> {
    settings().cloned()
}

fn settings_or_warn(lookup: impl Fn(&str) -> Option<String>) -> Option<LlmSettings> {
    match settings_from_env(lookup) {
        Ok(settings) => settings,
        Err(problem) => {
            eprintln!("watt: {problem}. The mock agent is used instead.");
            None
        }
    }
}

static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

/// The one HTTP client shared by every conversation.
pub fn shared_client() -> Result<&'static reqwest::Client, String> {
    CLIENT.get_or_init(|| llm::http_client().map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())
}

/// A new conversation id: "ses_" and 32 lowercase hex digits from the OS random source.
pub fn new_conversation_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("ses_{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

/// What the model is told about a conversation that was resumed from storage: the shell is a new session.
const RESUME_NOTE: &str = "This conversation was resumed from storage. The shell is the current session: its \
directory, environment and running jobs may differ from earlier in this conversation, so re-check before relying on \
earlier output.";
/// Added when the saved copy was made in the middle of a turn: the app ended before that turn finished.
const RESUME_NOTE_INTERRUPTED: &str = "The previous session ended abruptly (crash, quit or kill) in the middle \
of a turn, so the last request may be unfinished or half done: verify what actually happened before continuing.";

/// The resume note for a restored conversation. `in_turn` is the stored flag (see `StoredConversation::in_turn`).
fn resume_note_for(in_turn: bool) -> String {
    if in_turn { format!("{RESUME_NOTE} {RESUME_NOTE_INTERRUPTED}") } else { RESUME_NOTE.to_string() }
}

/// One session's conversation: the id sent to the provider, the history, the todo list, and the note the next turn
/// gets when the conversation was resumed. `reset` starts a new one.
pub struct Conversation {
    id: Mutex<String>,
    history: Arc<Mutex<History>>,
    todos: TodoList,
    resume_note: Mutex<Option<String>>,
    /// The models that refused images here. Kept by `reset` and `restore`: it is a fact about the model, not the chat.
    no_vision: NoVisionModels,
    /// How far this conversation's estimates were from the provider's counts. In memory only; reset by `reset` and
    /// `restore`, since it belongs to the content it was learned from.
    calibration: Calibration,
    /// The context limits that overflow errors taught, per model id. Kept by `reset` and `restore`, like the no-vision memo:
    /// it is a fact about the model.
    learned_limits: LearnedLimits,
    /// The usage last emitted for this conversation, kept so a saved conversation can show its meter when it is restored.
    /// Cleared by `reset`; `restore` clears it too, and the caller then sets the saved one (see `set_last_usage`).
    last_usage: LastUsage,
}

impl Conversation {
    pub fn new() -> io::Result<Self> {
        Ok(Conversation {
            id: Mutex::new(new_conversation_id()?),
            history: Arc::new(Mutex::new(History::new())),
            todos: Arc::new(Mutex::new(Vec::new())),
            resume_note: Mutex::new(None),
            no_vision: Arc::new(Mutex::new(HashSet::new())),
            calibration: Arc::new(Mutex::new(Calibrator::default())),
            learned_limits: Arc::new(Mutex::new(HashMap::new())),
            last_usage: Arc::new(Mutex::new(None)),
        })
    }

    pub fn id(&self) -> String {
        lock_string(&self.id).clone()
    }

    pub fn history(&self) -> Arc<Mutex<History>> {
        self.history.clone()
    }

    pub fn todos(&self) -> TodoList {
        self.todos.clone()
    }

    /// The models that refused images in this conversation (see `LlmAgent::with_no_vision`).
    pub fn no_vision(&self) -> NoVisionModels {
        self.no_vision.clone()
    }

    /// The calibration of this conversation's estimates (see `LlmAgent::with_calibration`).
    pub fn calibration(&self) -> Calibration {
        self.calibration.clone()
    }

    /// The context limits learned for this conversation's models (see `LlmAgent::with_learned_limits`).
    pub fn learned_limits(&self) -> LearnedLimits {
        self.learned_limits.clone()
    }

    /// The usage last emitted for this conversation (see `LlmAgent::with_last_usage`).
    pub fn last_usage(&self) -> LastUsage {
        self.last_usage.clone()
    }

    /// Sets the usage the conversation shows, for a conversation restored from storage (see `restore`).
    pub fn set_last_usage(&self, usage: Option<ContextUsage>) {
        *lock_usage(&self.last_usage) = usage;
    }

    /// Clears the history, the todo list, any pending resume note, the calibration and the last usage, and rotates the id.
    /// The caller makes sure no turn is running.
    pub fn reset(&self) -> io::Result<()> {
        let id = new_conversation_id()?;
        lock(&self.history).clear();
        lock_todos(&self.todos).clear();
        *lock_note(&self.resume_note) = None;
        *lock_calibration(&self.calibration) = Calibrator::default();
        *lock_usage(&self.last_usage) = None;
        *lock_string(&self.id) = id;
        Ok(())
    }

    /// Makes a saved conversation the live one: its id, history and todo list replace the current ones. The history and
    /// todo objects stay the same, so holders of `history()` and `todos()` see the new contents. The next turn gets a
    /// resume note (see `take_resume_note`). `in_turn` is the stored flag, which adds the interrupted-turn sentence.
    /// The caller makes sure no turn is running.
    pub fn restore(&self, id: String, history: History, todos: Vec<Todo>, in_turn: bool) {
        *lock_string(&self.id) = id;
        *lock(&self.history) = history;
        *lock_todos(&self.todos) = todos;
        *lock_note(&self.resume_note) = Some(resume_note_for(in_turn));
        // The restored conversation is a different content: the calibration learned from the live one does not carry over.
        *lock_calibration(&self.calibration) = Calibrator::default();
        // The last usage belongs to the content it was measured on. The caller sets the saved one (see `set_last_usage`).
        *lock_usage(&self.last_usage) = None;
    }

    /// Makes a new conversation the live one: its id, history and todo list replace the current ones. The history and
    /// todo objects stay the same, so holders of `history()` and `todos()` see the new contents. Unlike `restore`,
    /// there is no resume note: the new conversation continues the work, so the next turn gets none. The calibration
    /// and the last usage belonged to the old content and do not carry over. The caller makes sure no turn is running.
    pub fn adopt(&self, id: String, history: History, todos: Vec<Todo>) {
        *lock_string(&self.id) = id;
        *lock(&self.history) = history;
        *lock_todos(&self.todos) = todos;
        *lock_note(&self.resume_note) = None;
        *lock_calibration(&self.calibration) = Calibrator::default();
        *lock_usage(&self.last_usage) = None;
    }

    /// Another attached session continues this conversation: the id is copied and the history, todo list,
    /// no-vision memo, calibration, learned limits and last usage are shared, so both conversations see the same
    /// live state. The resume note is copied, so a note no turn has taken yet is still taken once.
    pub fn follow(&self) -> Self {
        Conversation {
            id: Mutex::new(lock_string(&self.id).clone()),
            history: self.history.clone(),
            todos: self.todos.clone(),
            resume_note: Mutex::new(lock_note(&self.resume_note).clone()),
            no_vision: self.no_vision.clone(),
            calibration: self.calibration.clone(),
            learned_limits: self.learned_limits.clone(),
            last_usage: self.last_usage.clone(),
        }
    }

    /// The resume note, once: a turn that calls the model takes it, and the next turn gets none. `None` for a
    /// conversation that was not resumed, or after a turn has taken it.
    pub fn take_resume_note(&self) -> Option<String> {
        lock_note(&self.resume_note).take()
    }
}

/// A save at a checkpoint of the turn. It gets the agent's redactor, which the save uses to scrub what it writes.
pub type Checkpoint = Box<dyn FnMut(&Redactor) + Send>;

/// One model request as it is sent: the system text it is built from (with the trim note when older content was left
/// out), the wire messages, and whether anything was left out.
struct WireRequest {
    system: String,
    messages: Vec<Value>,
    trimmed: bool,
}

/// A request that does not fit the context limit even with every trim: the estimate of the smallest request, and the
/// tokens available to a request.
struct TooLarge {
    need: u32,
    available: u32,
}

impl TooLarge {
    fn text(&self) -> String {
        format!(
            "This conversation no longer fits the model's context window (about {} tokens needed, {} available). Start a new \
             conversation, or raise the context limit in Settings.",
            self.need, self.available
        )
    }
}

/// The agent for one turn of a conversation: the provider config with this conversation's headers, the redactor, and
/// the shared history and cancel token.
pub struct LlmAgent {
    client: reqwest::Client,
    cfg: ProviderConfig,
    model: String,
    max_tokens: Option<u32>,
    stream: bool,
    /// The selected variant's request overlay, sent with every model call (see `LlmSettings::extra`).
    extra: Option<serde_json::Map<String, Value>>,
    redactor: Redactor,
    history: Arc<Mutex<History>>,
    /// The conversation's id, which names the directory its attachment files are read from.
    conversation_id: String,
    todos: TodoList,
    cancel: Arc<TurnCancel>,
    system: String,
    tools: Vec<Value>,
    /// The resume note of a restored conversation, for this turn's model calls (see `with_resume_note`).
    resume_note: Option<String>,
    /// The workspace root's AGENTS.md for this turn (see `with_project_rules`).
    project_rules: Option<String>,
    /// Saves the conversation at each checkpoint (see `with_checkpoint`).
    checkpoint: Option<Checkpoint>,
    /// The caps this turn runs under (see `with_limits`).
    limits: LimitsSettings,
    /// Where the images of attached messages are read from (see `with_attachments`). None: they are not sent.
    attachments: Option<AttachmentStore>,
    /// The models that refused images, by id (see `with_no_vision`).
    no_vision: NoVisionModels,
    /// The context limit of the model, from the settings (see `LlmSettings::context_limit`).
    context_limit: Option<u32>,
    /// The conversation's calibration (see `with_calibration`).
    calibration: Calibration,
    /// The context limits learned from overflow errors, by model id (see `with_learned_limits`).
    learned_limits: LearnedLimits,
    /// The usage last emitted, shared with the conversation (see `with_last_usage`).
    last_usage: LastUsage,
}

impl LlmAgent {
    pub fn new(
        client: &reqwest::Client,
        settings: LlmSettings,
        conversation_id: &str,
        history: Arc<Mutex<History>>,
        todos: TodoList,
        cancel: Arc<TurnCancel>,
    ) -> Self {
        let redactor = settings.redactor(conversation_id);
        LlmAgent {
            client: client.clone(),
            cfg: settings.provider_config(conversation_id),
            model: settings.model,
            max_tokens: settings.max_tokens,
            stream: settings.stream,
            extra: settings.extra,
            redactor,
            history,
            conversation_id: conversation_id.to_string(),
            todos,
            cancel,
            system: prompt::system_prompt(),
            tools: prompt::tool_definitions(),
            resume_note: None,
            project_rules: None,
            checkpoint: None,
            limits: LimitsSettings::default(),
            attachments: None,
            no_vision: Arc::new(Mutex::new(HashSet::new())),
            context_limit: settings.context_limit,
            calibration: Arc::new(Mutex::new(Calibrator::default())),
            learned_limits: Arc::new(Mutex::new(HashMap::new())),
            last_usage: Arc::new(Mutex::new(None)),
        }
    }

    /// The caps this turn runs under: the saved limits, read per user message. The default is the saved default.
    pub fn with_limits(mut self, limits: LimitsSettings) -> Self {
        self.limits = limits;
        self
    }

    /// The attachment store the images of this conversation are read from when a model call sends them. Without it,
    /// those images are reported as no longer available.
    pub fn with_attachments(mut self, store: AttachmentStore) -> Self {
        self.attachments = Some(store);
        self
    }

    /// The memo of models that refused images, shared with the conversation (see `Conversation::no_vision`). Without it,
    /// the memo lasts for this agent only. A model in the memo is sent its images as placeholders, with no call that
    /// would fail first.
    pub fn with_no_vision(mut self, models: NoVisionModels) -> Self {
        self.no_vision = models;
        self
    }

    /// The calibration shared with the conversation (see `Conversation::calibration`). Without it, the calibration lasts
    /// for this agent only. Each reply that reports its prompt size teaches it, and the next request is estimated with it.
    pub fn with_calibration(mut self, calibration: Calibration) -> Self {
        self.calibration = calibration;
        self
    }

    /// The context limits learned from overflow errors, shared with the conversation (see `Conversation::learned_limits`).
    /// Without it, what is learned lasts for this agent only. The limit of a model is the smaller of the configured one and
    /// the learned one, so after an overflow the model's later requests are trimmed before they are sent.
    pub fn with_learned_limits(mut self, limits: LearnedLimits) -> Self {
        self.learned_limits = limits;
        self
    }

    /// The usage last emitted, shared with the conversation (see `Conversation::last_usage`). Without it, the usage lasts
    /// for this agent only. Every usage event this agent emits is also kept here.
    pub fn with_last_usage(mut self, last: LastUsage) -> Self {
        self.last_usage = last;
        self
    }

    /// The note about a resumed conversation (from `Conversation::take_resume_note`). It goes into the system message
    /// of every model call of this turn and is never stored in the history.
    pub fn with_resume_note(mut self, note: Option<String>) -> Self {
        self.resume_note = note;
        self
    }

    /// The workspace root's AGENTS.md for this turn. It goes into the system message of every model
    /// call right after the base prompt and is never stored in the history. Missing or unreadable
    /// means no rules.
    pub fn with_project_rules(mut self, rules: Option<String>) -> Self {
        self.project_rules = rules;
        self
    }

    /// Calls `save` at each checkpoint: once the user message is recorded, once a reply's tool calls are recorded
    /// but before they run, and after each completed round of tool calls. The history is paired at the first and last
    /// of those points; the middle one may hold calls without results yet (a restart then repairs them into closed
    /// calls, see `History::from_messages`). The end-of-turn save is the caller's, after the turn returns.
    pub fn with_checkpoint(mut self, save: impl FnMut(&Redactor) + Send + 'static) -> Self {
        self.checkpoint = Some(Box::new(save));
        self
    }

    /// The redactor for this turn's secrets. The persistence layer uses it to scrub what it writes to disk.
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// Runs the checkpoint save, if one is set. Only called where the history is paired.
    fn save_checkpoint(&mut self) {
        if let Some(save) = self.checkpoint.as_mut() {
            save(&self.redactor);
        }
    }

    /// The system text of one model call: the base prompt, then the notes and blocks that apply to this call. They are
    /// added here only, and none of them is stored in the history. The todo block comes last.
    fn system_text(&self, ctx: &AgentCtx<'_>, finished: Option<&str>) -> String {
        let mut system = self.system.clone();
        if let Some(rules) = &self.project_rules {
            system = format!("{system}\n\n{}", instructions::note(rules));
        }
        if let Some(note) = &self.resume_note {
            system = format!("{system}\n\n{note}");
        }
        if let Some(note) = location_note(&ctx.top_level(), &self.redactor) {
            system = format!("{system}\n\n{note}");
        }
        // The agent's own command, when it is open, is described by its note. The foreground note is for any other program.
        let foreground = ctx.foreground();
        let open = ctx.open_agent_records();
        if let Some(note) = agent_record_note(&open, foreground.as_ref(), &self.redactor) {
            system = format!("{system}\n\n{note}");
        }
        let own_command_on_top = open.iter().any(|rec| rec.on_top);
        if !own_command_on_top && let Some(note) = foreground_note(foreground.as_ref(), &self.redactor) {
            system = format!("{system}\n\n{note}");
        }
        if let Some(note) = finished {
            system = format!("{system}\n\n{note}");
        }
        if let Some(block) = todo::prompt_block(&lock_todos(&self.todos)) {
            system = format!("{system}\n\n{block}");
        }
        system
    }

    /// One model call. Streams text as it arrives, or waits for the whole reply. Returns None when the call was
    /// cancelled or failed; a failure has already been shown to the user. `finished` is the turn's finished note (see
    /// `finished_since_last_turn`), given to the first call only. Every request is fitted to the context limit first
    /// (see `fit`); one that does not fit is not sent, and the turn ends with its notice.
    ///
    /// Two kinds of retry are possible, each at most once, and only before any text was streamed. An overflow (see
    /// `context::is_context_overflow`) is retried with a smaller limit (see `overflow_retry_limit`). A request with images
    /// that the model refuses (see `rejects_images`) is sent once more with the images replaced by notes. An overflow is
    /// never taken for a refusal of images. The stored history is not changed. The retries are part of this one call: the
    /// caps count replies that ask for tools (see `charge`), so a failed attempt is never counted, and the reply that comes
    /// back is counted once, as any other reply.
    fn ask_model(&self, ctx: &AgentCtx<'_>, finished: Option<&str>) -> Option<AssistantReply> {
        let system = self.system_text(ctx, finished);
        // A model that refused images earlier in this conversation is not asked again: its images are replaced at once.
        let mut images = !lock_models(&self.no_vision).contains(&self.model);
        let mut limit = self.effective_limit();
        let (mut overflow_retry, mut image_retry) = (true, true);
        // Set when the images were refused: a reply to the retry is then remembered as refusing them, and is noticed.
        let mut image_refused = false;
        // The error of the request that asked for the image retry. It is what the user sees if that retry fails too.
        let mut asker_error: Option<LlmError> = None;
        loop {
            let mut resolver = if images {
                Resolver::new(self.attachments.as_ref(), &self.conversation_id)
            } else {
                Resolver::without_images()
            };
            let request = self.fit_or_say(ctx, limit, &system, &mut resolver)?;
            let sent_images = has_image_part(&request.messages);
            let estimate = self.estimate(&request);
            let (result, streamed) = self.send(ctx, request);
            match result {
                Ok(reply) => {
                    if image_refused {
                        lock_models(&self.no_vision).insert(self.model.clone());
                        let reply = self.finish_call(ctx, Ok(reply));
                        ctx.say(NO_VISION_NOTICE);
                        return reply;
                    }
                    return self.finish_call(ctx, Ok(reply));
                }
                Err(LlmError::Cancelled) => return None,
                Err(e) if overflow_retry && !streamed && context::is_context_overflow(&e) => {
                    overflow_retry = false;
                    asker_error = None;
                    let Some(smaller) = self.overflow_retry_limit(&e, estimate, limit) else {
                        return self.finish_call(ctx, Err(e));
                    };
                    // `smaller` is below the limit this request was fitted to, which is the effective one, so it only lowers it.
                    lock_learned(&self.learned_limits).insert(self.model.clone(), smaller);
                    limit = Some(smaller);
                }
                Err(e)
                    if image_retry
                        && !streamed
                        && sent_images
                        && rejects_images(&e)
                        && !context::is_context_overflow(&e) =>
                {
                    image_retry = false;
                    images = false;
                    image_refused = true;
                    asker_error = Some(e);
                }
                Err(e) => return self.finish_call(ctx, Err(asker_error.take().unwrap_or(e))),
            }
        }
    }

    /// The context limit that applies now: the smaller of the configured one and the one learned for this model from an
    /// overflow. Either may be unknown; None only when both are.
    fn effective_limit(&self) -> Option<u32> {
        let learned = lock_learned(&self.learned_limits).get(&self.model).copied();
        match (self.context_limit, learned) {
            (Some(configured), Some(learned)) => Some(configured.min(learned)),
            (configured, learned) => configured.or(learned),
        }
    }

    /// The tokens kept for the reply: `max_tokens`, or the default.
    fn reserve(&self) -> u32 {
        self.max_tokens.unwrap_or(context::DEFAULT_OUTPUT_RESERVE)
    }

    /// The calibrated estimate of one request, as the usage event reports it.
    fn estimate(&self, request: &WireRequest) -> u32 {
        let breakdown = context::breakdown(&request.system, &self.tools, &request.messages);
        context::usage(&breakdown, &lock_calibration(&self.calibration), None, None).used
    }

    /// The limit for the one retry after an overflow: the limit the provider's error names, or else the one that cuts the
    /// failed request to 70% of its estimate (see `trim::overflow_limit`). `fitted` is the limit the failed request was
    /// fitted to. None when the new limit is not below it: the retry would be the same request, so it is not made.
    fn overflow_retry_limit(&self, e: &LlmError, estimate: u32, fitted: Option<u32>) -> Option<u32> {
        let named = match e {
            LlmError::Http { message, .. } | LlmError::Provider { message, .. } => context::limit_in_message(message),
            _ => None,
        };
        let limit = named.unwrap_or_else(|| trim::overflow_limit(estimate, self.reserve()));
        match fitted {
            Some(now) if limit >= now => None,
            _ => Some(limit),
        }
    }

    /// The request to send for `system` under `limit`: the stored history as wire messages, trimmed (see core/chat/trim)
    /// when the limit is known and the untrimmed request is over the budget. Nothing is trimmed without a known limit.
    /// Err when even the smallest request does not fit; the stored history is never changed.
    fn fit(&self, limit: Option<u32>, system: &str, resolver: &mut Resolver<'_>) -> Result<WireRequest, TooLarge> {
        let Some(limit) = limit else {
            return Ok(self.request(system, &TrimPlan::none(), resolver));
        };
        let reserve = self.reserve();
        let budget = trim::budget(limit, reserve);
        let calibrator = *lock_calibration(&self.calibration);
        let max_drop = lock(&self.history).turn_count().saturating_sub(1);
        let chosen = trim::select(budget, max_drop, |plan| {
            let request = self.request(system, plan, resolver);
            let breakdown = context::breakdown(&request.system, &self.tools, &request.messages);
            context::usage(&breakdown, &calibrator, Some(limit), self.max_tokens).used
        });
        match chosen {
            Ok(plan) => Ok(self.request(system, &plan, resolver)),
            Err(need) => Err(TooLarge { need, available: u32::try_from(budget.max(0)).unwrap_or(u32::MAX) }),
        }
    }

    /// `fit`, and when the request does not fit at all, its notice is shown and no request is made.
    fn fit_or_say(&self, ctx: &AgentCtx<'_>, limit: Option<u32>, system: &str, resolver: &mut Resolver<'_>) -> Option<WireRequest> {
        match self.fit(limit, system, resolver) {
            Ok(request) => Some(request),
            Err(too_large) => {
                ctx.say(self.redactor.redact(&too_large.text()));
                None
            }
        }
    }

    /// The request under `plan`: its system text, and its wire messages built with `resolver`.
    fn request(&self, system: &str, plan: &TrimPlan, resolver: &mut Resolver<'_>) -> WireRequest {
        let system = trim::system_for(system, plan);
        let messages = lock(&self.history).messages_with(&system, resolver, plan);
        WireRequest { system, messages, trimmed: !plan.is_none() }
    }

    /// One request, streamed or waited for, with the "thinking" state around it. Returns the result and whether any
    /// text was streamed to the user during the call. The context usage of the request is emitted before it is sent, and
    /// again with the provider's count if the reply reports one (see core/chat/context). The usage says whether older
    /// content was left out.
    fn send(&self, ctx: &AgentCtx<'_>, request: WireRequest) -> (Result<AssistantReply, LlmError>, bool) {
        let WireRequest { system, messages, trimmed } = request;
        let breakdown = context::breakdown(&system, &self.tools, &messages);
        let req = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: Some(self.tools.clone()),
            max_tokens: self.max_tokens,
            extra: self.extra.clone(),
        };
        let thinking = Cell::new(true);
        let streamed = Cell::new(false);
        // The raw streamed text, kept for the partial redaction below.
        let mut streamed_raw = String::new();
        ctx.emit(AgentEvent::Thinking { active: true });
        let before = ContextUsage {
            trimmed,
            ..context::usage(&breakdown, &lock_calibration(&self.calibration), self.context_limit, self.max_tokens)
        };
        self.emit_usage(ctx, before.clone());
        let result = if self.stream {
            let mut on_delta = |delta: Delta| {
                // Reasoning deltas are not shown yet, so they do not end the "thinking" state.
                if let Delta::Text(text) = delta
                    && !text.is_empty()
                {
                    stop_thinking(ctx, &thinking);
                    streamed.set(true);
                    streamed_raw.push_str(&text);
                    ctx.emit(AgentEvent::TextDelta { text: redact_stream_fragment(&self.redactor, &text) });
                }
            };
            block(llm::stream(&self.client, &self.cfg, &req, &mut on_delta, &self.cancel))
        } else {
            block(llm::complete(&self.client, &self.cfg, &req, &self.cancel))
        };
        stop_thinking(ctx, &thinking);
        // A stream that failed after showing text leaves a partial reply: the next request must see what the
        // user saw. Nothing is pushed when nothing streamed, when the stream finished, or when not streaming.
        if result.is_err() && streamed.get() && !streamed_raw.is_empty() {
            lock(&self.history).push_assistant(interrupted_message(&self.redactor, &streamed_raw));
        }
        if let Ok(reply) = &result
            && let Some(actual) = reply.usage.as_ref().map(|usage| usage.prompt_tokens).filter(|&n| n > 0)
        {
            let actual = u32::try_from(actual).unwrap_or(u32::MAX);
            lock_calibration(&self.calibration).observe(breakdown.total(), actual);
            self.emit_usage(ctx, context::with_actual(&before, actual));
        }
        (result, streamed.get())
    }

    /// Emits a context usage event and keeps it as the conversation's last usage (see `with_last_usage`).
    fn emit_usage(&self, ctx: &AgentCtx<'_>, usage: ContextUsage) {
        *lock_usage(&self.last_usage) = Some(usage.clone());
        ctx.emit(AgentEvent::ContextUsage(usage));
    }

    /// Ends one model call: a reply that was not streamed is said, and an error is shown. None when there is no reply.
    fn finish_call(&self, ctx: &AgentCtx<'_>, result: Result<AssistantReply, LlmError>) -> Option<AssistantReply> {
        match result {
            Ok(reply) => {
                if !self.stream && !reply.text.is_empty() {
                    ctx.say(self.redactor.redact(&reply.text));
                }
                Some(reply)
            }
            Err(LlmError::Cancelled) => None,
            Err(e) => {
                ctx.say(self.redactor.redact(&error_text(&e)));
                None
            }
        }
    }

    /// Runs the tool calls of one reply in order and records each result. Returns why the turn must stop, if it must.
    /// The calls after a stop are not run; the caller closes them in the history.
    fn run_calls(&self, calls: &[ToolCall], ctx: &AgentCtx<'_>, commands: &mut u32) -> Option<Stop> {
        for call in calls {
            if self.cancel.is_cancelled() {
                return Some(Stop::Cancelled);
            }
            let (content, denied) = self.run_call(call, ctx, commands);
            lock(&self.history).push_tool_result(&call.id, &content);
            if self.cancel.is_cancelled() {
                return Some(Stop::Cancelled);
            }
            if denied {
                return Some(Stop::Denied);
            }
        }
        None
    }

    /// Runs one call. Returns the model-facing text of its result and whether the user denied it.
    fn run_call(&self, call: &ToolCall, ctx: &AgentCtx<'_>, commands: &mut u32) -> (String, bool) {
        let name = call.name.as_str();
        let failed = |message: &str| (project(name, false, &Value::String(message.to_string()), &self.redactor), false);
        let Ok(args) = serde_json::from_str::<Value>(&call.arguments) else {
            return failed("arguments were not valid JSON");
        };
        let result = match name {
            "run_command" => {
                if *commands >= self.limits.max_commands_per_turn {
                    return failed("per-turn command limit reached; ask the user to continue");
                }
                *commands += 1;
                ctx.call_tool("run_command", args)
            }
            todo::TOOL_NAME => ctx.local_tool(name, &args, |args| self.write_todos(args, ctx)),
            // Chat-local like `todo_write`: answered by the user, never by the shell or an approval card. It counts
            // against neither the command cap nor the read names, and its result never stops the turn.
            questions::TOOL_NAME => ctx.ask_user(&args),
            _ if READ_NAMES.contains(&name) => ctx.read_tool(name, args),
            "request_integration" => ctx.call_tool("request_integration", args),
            "send_keys" => ctx.call_tool("send_keys", args),
            _ => {
                let shown: String = name.chars().take(NAME_LIMIT).collect();
                return failed(&format!("unknown tool `{shown}`"));
            }
        };
        let denied = matches!(&result, Err(e) if e.starts_with(DENIED));
        let content = match result {
            Ok(value) => project(name, true, &value, &self.redactor),
            Err(e) => project(name, false, &Value::String(e), &self.redactor),
        };
        (content, denied)
    }

    /// Validates a `todo_write` call and, when it is valid, replaces the conversation's list and reports it. A refused
    /// call leaves the list as it was. The error text is what the model reads back.
    fn write_todos(&self, args: &Value, ctx: &AgentCtx<'_>) -> Result<Value, String> {
        let items = todo::parse(args)?;
        *lock_todos(&self.todos) = items.clone();
        ctx.emit(AgentEvent::TodoUpdate { items: items.clone() });
        Ok(json!({"items": items}))
    }
}

impl Agent for LlmAgent {
    fn respond(&mut self, message: &UserMessage, ctx: &AgentCtx<'_>) {
        // The document text is redacted here, as it enters the history (see `user_content`).
        let content = user_content(&message.text, &message.attachments, &self.redactor);
        lock(&self.history).push_user_content(content);
        self.save_checkpoint();
        // The finished note is for this turn's first model call only: it says what finished since the last turn.
        let mut finished = finished_since_last_turn(&lock(&self.history).snapshot(), ctx);
        let mut commands = 0;
        let (mut general, mut polls) = (0, 0);
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            let Some(reply) = self.ask_model(ctx, finished.take().as_deref()) else {
                return;
            };
            lock(&self.history).push_assistant(reply.raw);
            if reply.tool_calls.is_empty() {
                return;
            }
            // The calls are on record but not yet run: a restart from this checkpoint loads the unanswered call, and
            // the repair closes it (see `History::from_messages`). The post-run checkpoint below still saves the results.
            self.save_checkpoint();
            let polling = is_polling(&ctx.open_agent_records(), &reply.tool_calls);
            if let Some(cap) = self.charge(polling, &mut general, &mut polls) {
                // The calls of the reply that reached the cap are not run, so nothing happens after the limit.
                lock(&self.history).close_pending(cap.reason());
                ctx.say(cap.notice());
                return;
            }
            if let Some(stop) = self.run_calls(&reply.tool_calls, ctx, &mut commands) {
                lock(&self.history).close_pending(stop.reason());
                if stop == Stop::Denied {
                    ctx.say(DENIED_NOTICE);
                }
                return;
            }
            self.save_checkpoint();
        }
    }
}

impl LlmAgent {
    /// Counts one reply that asks for tool calls, against the cap it belongs to: a polling reply against the polling cap
    /// only, any other reply against the model-call cap only. Returns the cap the count reached, if it reached one. A
    /// reply that reaches a cap is not run.
    fn charge(&self, polling: bool, general: &mut u32, polls: &mut u32) -> Option<Cap> {
        if polling {
            *polls += 1;
            return (*polls > self.limits.max_poll_calls).then_some(Cap::Polls(self.limits.max_poll_calls));
        }
        *general += 1;
        self.limits.model_call_cap().filter(|&cap| *general > cap).map(Cap::Model)
    }
}

/// True when a reply counts as polling: an agent-started command is still running, and every call in the reply is one that
/// only looks at it or types into it (wait, get_screen, send_keys).
fn is_polling(open: &[OpenAgentRecord], calls: &[ToolCall]) -> bool {
    !open.is_empty() && !calls.is_empty() && calls.iter().all(|call| POLL_NAMES.contains(&call.name.as_str()))
}

/// A cap that a reply reached. The turn ends there, and the reply's calls are not run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cap {
    /// The model-call cap, with its value.
    Model(u32),
    /// The polling cap, with its value.
    Polls(u32),
}

impl Cap {
    fn reason(self) -> &'static str {
        match self {
            Cap::Model(_) => LIMIT_REASON,
            Cap::Polls(_) => POLL_LIMIT_REASON,
        }
    }

    /// What the user is told. It names the cap and its value, and where to change it.
    fn notice(self) -> String {
        match self {
            Cap::Model(n) => format!("I stopped after {n} model calls in one turn. The limit can be changed in Settings > Limits."),
            Cap::Polls(n) => format!(
                "I stopped after {n} polling calls in one turn (wait, get_screen or send_keys while a command you started is \
                 running). The limit can be changed in Settings > Limits."
            ),
        }
    }
}

/// Why a turn stopped before the model answered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Denied,
    Cancelled,
}

impl Stop {
    fn reason(self) -> &'static str {
        match self {
            Stop::Denied => "denied by the user",
            Stop::Cancelled => "cancelled",
        }
    }
}

/// Runs an async adapter call on Tauri's runtime, so the shared client's connections stay on one runtime.
fn block<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

/// Emits `Thinking { active: false }` the first time it is called after `Thinking { active: true }`.
fn stop_thinking(ctx: &AgentCtx<'_>, active: &Cell<bool>) {
    if active.replace(false) {
        ctx.emit(AgentEvent::Thinking { active: false });
    }
}

/// True when a request carries an image part: an image the model is sent.
fn has_image_part(messages: &[Value]) -> bool {
    messages.iter().any(|m| m["content"].as_array().is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url")))
}

/// True for an error that a model gives for images it does not accept: HTTP 400, 404 or 422, or a provider error. Other
/// statuses and transport problems say nothing about the images.
fn rejects_images(e: &LlmError) -> bool {
    matches!(e, LlmError::Http { status: 400 | 404 | 422, .. } | LlmError::Provider { .. })
}

/// Redacts one streamed text fragment before it is shown. A fragment is redacted on its own, so a
/// credential split across two fragments is only caught by the whole-text redaction of the stored message.
fn redact_stream_fragment(redactor: &Redactor, text: &str) -> String {
    redactor.redact(text)
}

/// The assistant message stored when a streamed reply fails after text arrived: the accumulated text,
/// redacted as a whole, with a marker that the reply was interrupted. Redacting the whole text catches a
/// credential split across two deltas, which the per-delta redaction above cannot see.
fn interrupted_message(redactor: &Redactor, raw: &str) -> Value {
    json!({"role": "assistant", "content": format!("{}\n\n[reply interrupted]", redactor.redact(raw))})
}

/// The notice for a failed model call. Provider text is already free of the key (the adapter scrubs it).
fn error_text(e: &LlmError) -> String {
    match e {
        LlmError::Http { status, message, .. } => format!("The provider returned HTTP {status}: {message}"),
        LlmError::Provider { message, .. } => format!("The provider reported an error: {message}"),
        LlmError::Transport(detail) => format!("I could not reach the provider: {detail}"),
        LlmError::Parse(detail) => format!("I could not read the provider's reply: {detail}"),
        LlmError::TooLarge(detail) => format!("The provider's reply is too large: {detail}"),
        LlmError::Cancelled => "The request was cancelled.".to_string(),
    }
}

/// The note for one model call while the shell on top is a nested one (not the local machine). It names the shell, the user
/// and host its Hello gave, and says what the agent must not do there. None on the local shell. The label is the user's
/// command text, so the redactor removes what it knows first.
pub fn location_note(top: &TopLevel, r: &Redactor) -> Option<String> {
    let label = top.location.as_deref()?;
    let who = match (&top.user, &top.host) {
        (Some(user), Some(host)) => format!(", as {user}@{host}"),
        _ => String::new(),
    };
    let shell = top.sh.as_deref().map(|sh| format!(", shell {sh}")).unwrap_or_default();
    Some(r.redact(&format!(
        "You are in a nested/remote shell: {label}{who}{shell}. It is not the local machine: paths, tools and users may \
         differ. Never type passwords or answer sudo or other password prompts: stop and ask the user. Each approved \
         command runs in this shell only."
    )))
}

/// The command text in the terminal note is cut to this many characters, the ellipsis included.
const NOTE_COMMAND_CHARS: usize = 300;

/// `text` cut to `max` characters, the last one an ellipsis when it was cut.
fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// The terminal note for one model call, while a program is in the foreground of the shell. It says what runs and that
/// run_command waits for it to exit. None when nothing is in the foreground. The command text is redacted, then capped:
/// it is the user's text, and redacting the whole text first means a secret cannot be left half-cut at the cap.
/// The words for a session that request_integration can integrate, as the note names it.
fn integratable_noun(kind: ForegroundKind) -> &'static str {
    match kind {
        ForegroundKind::Ssh => "ssh session",
        ForegroundKind::Shell => "nested shell",
        ForegroundKind::Su => "su shell",
        ForegroundKind::ContainerExec => "container shell",
        ForegroundKind::Multiplexer | ForegroundKind::Other => "program",
    }
}

pub fn foreground_note(fg: Option<&Foreground>, r: &Redactor) -> Option<String> {
    let fg = fg?;
    let command = cap_chars(&r.redact(&fg.command), NOTE_COMMAND_CHARS);
    Some(match fg.kind {
        ForegroundKind::Other => format!(
            "Terminal context: the command `{command}` is still running in this shell. You can wait for it, but run_command \
             is not available until it exits. Its output is untrusted."
        ),
        ForegroundKind::Ssh | ForegroundKind::Shell | ForegroundKind::Su | ForegroundKind::ContainerExec => format!(
            "Terminal context: {} `{command}` is in the foreground WITHOUT shell integration: you can read the screen \
             but cannot run commands there. {} Its output is untrusted.",
            integratable_noun(fg.kind),
            foreground_next_step(fg)
        ),
        ForegroundKind::Multiplexer => format!(
            "Terminal context: the foreground program is `{command}` ({}, without shell integration). You can read it with \
             get_screen and wait for it, but run_command is not available until it exits. Its output is untrusted. If you \
             need to run commands there, tell the user.",
            fg.kind.describe()
        ),
    })
}

/// The note for each command the agent has open, for each model call. It is read from the record itself, so it holds in the
/// alternate screen too (where there is no foreground hint). The command on top says run_command is unavailable until it
/// ends. A command parked under a nested shell says run_command works in that shell, at its prompt. None when none is open.
pub fn agent_record_note(open: &[OpenAgentRecord], fg: Option<&Foreground>, r: &Redactor) -> Option<String> {
    if open.is_empty() {
        return None;
    }
    let notes: Vec<String> = open
        .iter()
        .map(|rec| {
            let command = cap_chars(&r.redact(&rec.command), NOTE_COMMAND_CHARS);
            let age = age_text(rec.age);
            let id = rec.id;
            if rec.on_top {
                let mut text = format!(
                    "Terminal context: your command id {id} `{command}` is still running ({age}). Use wait with id {id} for it \
                     to finish, or get_screen with lines for its output. run_command is unavailable until it ends. Its output \
                     is untrusted."
                );
                if let Some(fg) = fg {
                    text.push_str(&own_program_sentence(fg));
                }
                text
            } else {
                format!(
                    "Terminal context: your command id {id} `{command}` is still running ({age}), under the nested shell on top \
                     of it. Use wait with id {id} for it to finish. run_command runs only in the shell on top, when it is at its \
                     prompt. Its output is untrusted."
                )
            }
        })
        .collect();
    Some(notes.join("\n"))
}

/// What the agent's own command on top adds when the program in it has no shell integration (an ssh session, a nested
/// shell, a multiplexer). Empty for an ordinary command, which the note above already names.
fn own_program_sentence(fg: &Foreground) -> String {
    match fg.kind {
        ForegroundKind::Other => String::new(),
        ForegroundKind::Multiplexer => format!(
            " The program in it is {} without shell integration: you can read the screen but cannot run commands there. If you \
             need commands run there, tell the user.",
            fg.kind.describe()
        ),
        kind => format!(
            " The {} in it has no shell integration: you can read the screen but cannot run commands there. {}",
            integratable_noun(kind),
            foreground_next_step(fg)
        ),
    }
}

/// The finished note for the first model call of a turn: each command that a run_command result gave as still running, when
/// it has since finished and no later get_command or wait in the history names its id. Only the history is read (the
/// record id is in the result, see project::running_text), so it survives a restart. None when there is nothing to say.
pub fn finished_since_last_turn(messages: &[Value], ctx: &AgentCtx<'_>) -> Option<String> {
    let lines: Vec<String> = yielded_run_ids(messages)
        .into_iter()
        .filter(|&(at, id)| !resolved_after(messages, at, id))
        .filter_map(|(_, id)| {
            let summary = ctx.finished_summary(id)?;
            let exit = summary.exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string());
            Some(format!("finished since your last turn: id {id} exit {exit}, use get_command with id {id} to read it."))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// The run_command results in `messages` that gave a running command: each message index with the record id it carries.
fn yielded_run_ids(messages: &[Value]) -> Vec<(usize, u64)> {
    let names = call_names(messages);
    messages
        .iter()
        .enumerate()
        .filter_map(|(at, m)| {
            if m["role"] != "tool" {
                return None;
            }
            let call = m["tool_call_id"].as_str()?;
            if names.get(call).map(String::as_str) != Some("run_command") {
                return None;
            }
            // The result is one line of compact JSON; a note (nested shells) may follow it on the next line.
            let result: Value = serde_json::from_str(m["content"].as_str()?.lines().next()?).ok()?;
            if result["outcome"] != "running" {
                return None;
            }
            Some((at, result["record_id"].as_u64()?))
        })
        .collect()
}

/// The name of each tool call the assistant made, by call id.
fn call_names(messages: &[Value]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for m in messages.iter().filter(|m| m["role"] == "assistant") {
        for call in m["tool_calls"].as_array().into_iter().flatten() {
            if let (Some(id), Some(name)) = (call["id"].as_str(), call["function"]["name"].as_str()) {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }
    names
}

/// True when a get_command or wait call after message `at` names record `id` in its arguments.
fn resolved_after(messages: &[Value], at: usize, id: u64) -> bool {
    messages[at + 1..]
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["tool_calls"].as_array().into_iter().flatten())
        .any(|call| {
            let name = call["function"]["name"].as_str().unwrap_or_default();
            let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
            matches!(name, "get_command" | "wait") && args["id"].as_u64() == Some(id)
        })
}

fn lock(m: &Mutex<History>) -> MutexGuard<'_, History> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_string(m: &Mutex<String>) -> MutexGuard<'_, String> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_todos(m: &TodoList) -> MutexGuard<'_, Vec<Todo>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_note(m: &Mutex<Option<String>>) -> MutexGuard<'_, Option<String>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_models(m: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_calibration(m: &Mutex<Calibrator>) -> MutexGuard<'_, Calibrator> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_learned(m: &Mutex<HashMap<String, u32>>) -> MutexGuard<'_, HashMap<String, u32>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_usage(m: &Mutex<Option<ContextUsage>>) -> MutexGuard<'_, Option<ContextUsage>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The nested-shell note is present only on a nested level, and names its label, user, host and shell.
    #[test]
    fn the_location_note_is_only_for_a_nested_shell() {
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        assert_eq!(location_note(&TopLevel::default(), &r), None, "the local shell has no note");
        let remote = TopLevel {
            key: 1,
            location: Some("sshpass -p hunter2-secret ssh ubuntu@lidia".into()),
            user: Some("ubuntu".into()),
            host: Some("lidia".into()),
            sh: Some("zsh".into()),
        };
        let note = location_note(&remote, &r).expect("a note on a nested level");
        assert!(
            note.starts_with("You are in a nested/remote shell: sshpass -p [REDACTED] ssh ubuntu@lidia, as ubuntu@lidia, shell zsh. It is not the local machine"),
            "{note}"
        );
        assert!(note.contains("Never type passwords"), "{note}");
        assert!(note.contains("Each approved command runs in this shell only."), "{note}");
        assert!(!note.contains("hunter2-secret"), "{note}");
        // Without the Hello, the note still names the shell's label and says what it is.
        let bare = TopLevel { key: 2, location: Some("ssh h".into()), ..TopLevel::default() };
        let note = location_note(&bare, &r).expect("a note");
        assert!(note.starts_with("You are in a nested/remote shell: ssh h. It is not the local machine"), "{note}");
    }

    /// A lookup over the given pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    /// A complete set of required variables, plus `extra` (an extra value for a name replaces the base one).
    fn with_env(extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let mut map: HashMap<String, String> = [
            (ENV_URL, "https://openrouter.ai/api/v1/chat/completions"),
            (ENV_KEY, "sk-test-SECRETVALUE-0123456789"),
            (ENV_MODEL, "deepseek/deepseek-v4.1-flash"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in extra {
            map.insert(k.to_string(), v.to_string());
        }
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn missing_variables_select_the_mock_agent() {
        assert!(settings_from_env(env(&[])).unwrap().is_none());
        // Any one or two of the three is still not enough.
        assert!(settings_from_env(env(&[(ENV_URL, "https://x.test/v1/chat/completions")])).unwrap().is_none());
        assert!(settings_from_env(env(&[(ENV_KEY, "sk-test-SECRETVALUE"), (ENV_MODEL, "m")])).unwrap().is_none());
        // Empty counts as missing.
        assert!(settings_from_env(with_env(&[(ENV_MODEL, "   ")])).unwrap().is_none());
    }

    #[test]
    fn all_three_variables_give_defaults_for_the_optional_ones() {
        let s = settings_from_env(with_env(&[])).unwrap().unwrap();
        assert_eq!(s.chat_url, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(s.model, "deepseek/deepseek-v4.1-flash");
        assert!(s.stream);
        assert_eq!(s.max_tokens, None);
        assert!(s.headers.is_empty());
    }

    #[test]
    fn stream_and_max_tokens_are_parsed_and_checked() {
        assert!(!settings_from_env(with_env(&[(ENV_STREAM, "0")])).unwrap().unwrap().stream);
        assert!(settings_from_env(with_env(&[(ENV_STREAM, "1")])).unwrap().unwrap().stream);
        let err = settings_from_env(with_env(&[(ENV_STREAM, "yes")])).unwrap_err();
        assert_eq!(err, "WATT_LLM_STREAM must be 0 or 1");
        assert_eq!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "512")])).unwrap().unwrap().max_tokens, Some(512));
        assert!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "0")])).is_err());
        assert!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "lots")])).is_err());
    }

    #[test]
    fn a_url_without_http_is_refused_without_echoing_it() {
        let err = settings_from_env(env(&[
            (ENV_URL, "ftp://user:hunter2@host/x"),
            (ENV_KEY, "sk-test-SECRETVALUE"),
            (ENV_MODEL, "m"),
        ]))
        .unwrap_err();
        assert_eq!(err, "WATT_LLM_URL must start with https:// or http://");
    }

    #[test]
    fn headers_are_split_by_line_and_semicolon_and_the_placeholder_is_resolved() {
        let s = settings_from_env(with_env(&[(
            ENV_HEADERS,
            "x-opencode-session: {session_id}\nX-Title:  Watt Terminal ; X-Extra: a-{session_id}-b",
        )]))
        .unwrap()
        .unwrap();
        assert_eq!(
            s.headers,
            vec![
                ("x-opencode-session".to_string(), "{session_id}".to_string()),
                ("X-Title".to_string(), "Watt Terminal".to_string()),
                ("X-Extra".to_string(), "a-{session_id}-b".to_string()),
            ]
        );
        let resolved = s.resolved_headers("ses_abc");
        assert_eq!(resolved[0].1, "ses_abc");
        assert_eq!(resolved[2].1, "a-ses_abc-b");
    }

    #[test]
    fn a_bad_header_line_is_a_clear_startup_error_that_hides_the_value() {
        let err = settings_from_env(with_env(&[(
            ENV_HEADERS,
            "X-Good: 1\nthis line has the value secret-value-xyz but no colon",
        )]))
        .unwrap_err();
        assert_eq!(err, "WATT_LLM_HEADERS entry 2 is not `Name: value`");
        assert!(!err.contains("secret-value-xyz"));

        let bad_name = settings_from_env(with_env(&[(ENV_HEADERS, "bad name: v")])).unwrap_err();
        assert!(bad_name.contains("entry 1 has an invalid header name"), "{bad_name}");
        let empty_name = settings_from_env(with_env(&[(ENV_HEADERS, ": value")])).unwrap_err();
        assert!(empty_name.contains("entry 1 is not `Name: value`"), "{empty_name}");
    }

    #[test]
    fn a_bad_header_falls_back_to_the_mock_agent() {
        let settings = settings_or_warn(with_env(&[(ENV_HEADERS, "no colon here")]));
        assert!(settings.is_none());
        assert!(settings_or_warn(with_env(&[])).is_some());
    }

    #[test]
    fn debug_output_never_shows_the_key_or_header_values() {
        let s = settings_from_env(with_env(&[(ENV_HEADERS, "x-token: header-secret-123")])).unwrap().unwrap();
        let debug = format!("{s:?}");
        assert!(!debug.contains("SECRETVALUE"), "{debug}");
        assert!(!debug.contains("header-secret-123"), "{debug}");
        assert!(debug.contains("x-token"));
    }

    #[test]
    fn conversation_ids_have_the_expected_shape_and_differ() {
        let a = new_conversation_id().unwrap();
        let b = new_conversation_id().unwrap();
        for id in [&a, &b] {
            assert_eq!(id.len(), 36, "{id}");
            assert!(id.starts_with("ses_"));
            assert!(id[4..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{id}");
        }
        assert_ne!(a, b);
    }

    #[test]
    fn reset_clears_the_history_and_rotates_the_id() {
        let conversation = Conversation::new().unwrap();
        let before = conversation.id();
        lock(&conversation.history()).push_user("hello");
        conversation.reset().unwrap();
        assert_ne!(conversation.id(), before);
        assert!(lock(&conversation.history()).is_empty());
    }

    #[test]
    fn the_last_usage_is_cleared_by_reset_and_replaced_by_restore() {
        let usage = context::usage(&context::breakdown("abcd", &[], &[]), &Calibrator::default(), Some(100), None);
        let saved = ContextUsage { used: 7, ..usage.clone() };
        let conversation = Conversation::new().unwrap();
        *lock_usage(&conversation.last_usage()) = Some(usage);

        conversation.restore(conversation.id(), History::new(), Vec::new(), false);
        assert!(lock_usage(&conversation.last_usage()).is_none(), "a restore clears it until the caller sets the saved one");
        conversation.set_last_usage(Some(saved.clone()));
        assert_eq!(*lock_usage(&conversation.last_usage()), Some(saved.clone()));

        conversation.reset().unwrap();
        assert!(lock_usage(&conversation.last_usage()).is_none(), "a reset clears it");
    }

    #[test]
    fn a_restored_conversation_gives_its_resume_note_to_one_turn_only() {
        let conversation = Conversation::new().unwrap();
        assert!(conversation.take_resume_note().is_none(), "a new conversation has no note");

        conversation.restore(conversation.id(), History::new(), Vec::new(), false);
        assert_eq!(conversation.take_resume_note().as_deref(), Some(RESUME_NOTE));
        assert!(conversation.take_resume_note().is_none(), "taking the note clears it");

        conversation.restore(conversation.id(), History::new(), Vec::new(), true);
        let note = conversation.take_resume_note().expect("a note");
        assert!(note.starts_with(RESUME_NOTE) && note.ends_with(RESUME_NOTE_INTERRUPTED), "{note}");

        // A new conversation drops a note that no turn has taken yet.
        conversation.restore(conversation.id(), History::new(), Vec::new(), false);
        conversation.reset().unwrap();
        assert!(conversation.take_resume_note().is_none());
    }

    #[test]
    fn adopt_replaces_the_conversation_and_clears_the_note_calibration_and_usage() {
        let conversation = Conversation::new().unwrap();
        lock(&conversation.history()).push_user("old message");
        *lock_todos(&conversation.todos()) =
            vec![Todo { content: "old step".to_string(), status: todo::TodoStatus::Pending }];
        conversation.restore(conversation.id(), History::new(), Vec::new(), false);
        let usage = context::usage(&context::breakdown("abcd", &[], &[]), &Calibrator::default(), Some(100), None);
        conversation.set_last_usage(Some(usage));
        lock_calibration(&conversation.calibration()).observe(1000, 500);
        assert!(lock_calibration(&conversation.calibration()).observed());

        let mut history = History::new();
        history.push_user("new message");
        let todos = vec![Todo { content: "new step".to_string(), status: todo::TodoStatus::InProgress }];
        let id = new_conversation_id().unwrap();
        conversation.adopt(id.clone(), history, todos);

        assert_eq!(conversation.id(), id);
        assert_eq!(lock(&conversation.history()).snapshot().len(), 1);
        assert_eq!(lock(&conversation.history()).snapshot()[0]["content"], json!("new message"));
        assert_eq!(
            *lock_todos(&conversation.todos()),
            vec![Todo { content: "new step".to_string(), status: todo::TodoStatus::InProgress }]
        );
        assert!(conversation.take_resume_note().is_none(), "adopt leaves no resume note");
        assert_eq!(*lock_calibration(&conversation.calibration()), Calibrator::default());
        assert!(lock_usage(&conversation.last_usage()).is_none());
    }

    #[test]
    fn follow_shares_the_conversation_with_another_attached_session() {
        let conversation = Conversation::new().unwrap();
        let mut history = History::new();
        history.push_user("carried message");
        conversation.restore(
            conversation.id(),
            history,
            vec![Todo { content: "carried step".to_string(), status: todo::TodoStatus::InProgress }],
            false,
        );
        let usage = context::usage(&context::breakdown("abcd", &[], &[]), &Calibrator::default(), Some(100), None);
        conversation.set_last_usage(Some(usage.clone()));
        lock_calibration(&conversation.calibration()).observe(1000, 500);

        let carried = conversation.follow();
        assert_eq!(carried.id(), conversation.id(), "a restart keeps the conversation id");
        assert_eq!(carried.take_resume_note().as_deref(), Some(RESUME_NOTE), "the resume note is cloned");
        assert!(carried.take_resume_note().is_none(), "taking the note clears it");

        lock(&carried.history()).push_user("after restart");
        assert_eq!(lock(&conversation.history()).snapshot().len(), 2, "the history is shared");
        assert_eq!(
            *lock_todos(&conversation.todos()),
            vec![Todo { content: "carried step".to_string(), status: todo::TodoStatus::InProgress }],
            "the todo list is shared"
        );
        assert_eq!(*lock_usage(&carried.last_usage()), Some(usage), "the last usage is shared");
        assert!(lock_calibration(&carried.calibration()).observed(), "the calibration is shared");
    }

    fn hint(kind: ForegroundKind, command: &str) -> Foreground {
        Foreground { kind, command: command.to_string() }
    }

    /// The note is there only while a program is in the foreground, and it is the same text for every model call.
    #[test]
    fn the_terminal_note_is_absent_without_a_foreground_program() {
        let r = Redactor::new(Vec::new());
        assert_eq!(foreground_note(None, &r), None);
    }

    /// A shell without integration: the note names the session and the program, says the agent can read but not run
    /// commands there, and points at request_integration. The output is untrusted.
    #[test]
    fn the_terminal_note_names_a_program_without_integration() {
        let r = Redactor::new(Vec::new());
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        let note = foreground_note(Some(&ssh), &r).expect("a note for an ssh session");
        assert_eq!(
            note,
            "Terminal context: ssh session `ssh ubuntu@lidia` is in the foreground WITHOUT shell integration: you can read the \
             screen but cannot run commands there. Call request_integration to ask for it. Its output is untrusted."
        );
    }

    /// Each shell that request_integration can integrate is named in words, not by its kind code.
    #[test]
    fn the_terminal_note_names_each_integratable_session_in_words() {
        let r = Redactor::new(Vec::new());
        for (kind, words) in [
            (ForegroundKind::Ssh, "ssh session `ssh h`"),
            (ForegroundKind::Shell, "nested shell `ssh h`"),
            (ForegroundKind::Su, "su shell `ssh h`"),
            (ForegroundKind::ContainerExec, "container shell `ssh h`"),
        ] {
            let note = foreground_note(Some(&hint(kind, "ssh h")), &r).expect("a note");
            assert!(note.starts_with(&format!("Terminal context: {words} is in the foreground WITHOUT shell integration")), "{note}");
            assert!(note.contains("Call request_integration to ask for it."), "{note}");
        }
    }

    /// A remote command (`ssh host make`) is not something request_integration can integrate: the note says to wait for it.
    #[test]
    fn the_terminal_note_offers_request_integration_only_for_a_login() {
        let r = Redactor::new(Vec::new());
        let note = foreground_note(Some(&hint(ForegroundKind::Ssh, "ssh host make")), &r).expect("a note");
        assert!(!note.contains("request_integration"), "{note}");
        assert!(note.contains("Wait for it to end (wait)"), "{note}");
    }

    /// An ordinary running command gets a note too, with words that match it: it is still running in this shell.
    #[test]
    fn the_terminal_note_for_an_ordinary_command_does_not_call_it_unintegrated() {
        let r = Redactor::new(Vec::new());
        let sleep = hint(ForegroundKind::Other, "sleep 30");
        let note = foreground_note(Some(&sleep), &r).expect("a note while a command runs");
        assert!(note.contains("the command `sleep 30` is still running in this shell"), "{note}");
        assert!(!note.contains("without shell integration"), "{note}");
        assert!(note.contains("run_command is not available until it exits"), "{note}");
    }

    /// The command text is the user's, so the redactor removes what it knows before the note reaches the model.
    #[test]
    fn the_terminal_note_redacts_the_command() {
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        let ssh = hint(ForegroundKind::Ssh, "sshpass -p hunter2-secret ssh host");
        let note = foreground_note(Some(&ssh), &r).expect("a note");
        assert!(note.contains("`sshpass -p [REDACTED] ssh host`"), "{note}");
        assert!(!note.contains("hunter2-secret"), "{note}");
    }

    /// A long command is cut in the note to 300 characters, the last one an ellipsis, and the rest of it is not sent.
    #[test]
    fn the_terminal_note_caps_a_long_command() {
        let r = Redactor::new(Vec::new());
        let long = hint(ForegroundKind::Ssh, &format!("ssh {}", "a".repeat(1200)));
        let note = foreground_note(Some(&long), &r).expect("a note");
        let shown = format!("ssh {}…", "a".repeat(295));
        assert_eq!(shown.chars().count(), NOTE_COMMAND_CHARS);
        assert!(note.contains(&format!("`{shown}`")), "{note}");
        assert!(!note.contains(&"a".repeat(296)), "{note}");
    }

    /// The secret straddles the cut (it starts before character 299 and runs past it), and the text is still longer than
    /// the cap once redacted. The secret is removed before the cut, so no part of it reaches the note. Cutting first would
    /// leave `hunter2-s` in the note.
    #[test]
    fn the_terminal_note_redacts_before_it_caps() {
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        let text = format!("ssh {} hunter2-secret {}", "a".repeat(285), "b".repeat(50));
        let note = foreground_note(Some(&hint(ForegroundKind::Ssh, &text)), &r).expect("a note");
        assert!(note.contains('…'), "the command was not capped: {note}");
        assert!(!note.contains("hunter2"), "{note}");
        assert!(!note.contains("secret"), "{note}");
    }

    #[test]
    fn cap_chars_keeps_short_text_and_cuts_long_text_to_the_limit_with_an_ellipsis() {
        assert_eq!(cap_chars("abc", 3), "abc");
        assert_eq!(cap_chars("abcd", 3), "ab…");
        assert_eq!(cap_chars("日本語です", 3), "日本…", "characters, not bytes");
        assert_eq!(cap_chars(&"日".repeat(400), 300).chars().count(), 300);
    }

    #[test]
    fn error_notices_name_the_status_or_the_transport_problem() {
        let http = LlmError::Http { status: 400, code: Some(400), kind: None, message: "bad model".into() };
        assert_eq!(error_text(&http), "The provider returned HTTP 400: bad model");
        assert_eq!(
            error_text(&LlmError::Transport("could not connect: refused".into())),
            "I could not reach the provider: could not connect: refused"
        );
    }

    /// A streamed secret fragment is emitted redacted, never as written.
    #[test]
    fn a_streamed_secret_delta_is_emitted_redacted() {
        let redactor = Redactor::new(vec!["tok-value-123456".to_string()]);
        assert_eq!(
            redact_stream_fragment(&redactor, "the key is tok-value-123456 done"),
            "the key is [REDACTED] done"
        );
    }

    /// An interrupted stream leaves exactly one partial message, redacted as a whole, and the next request
    /// carries it. Per-delta redaction cannot see a secret split across two fragments; the whole-text
    /// redaction of the stored message can.
    #[test]
    fn an_interrupted_stream_leaves_one_partial_message_for_the_next_request() {
        let redactor = Redactor::new(vec!["tok-value-123456".to_string()]);
        let first = redact_stream_fragment(&redactor, "the key is tok-value-");
        let second = redact_stream_fragment(&redactor, "123456 done");
        assert!(
            (first.clone() + &second).contains("tok-value-123456"),
            "per-delta redaction cannot see the split secret: {first:?} + {second:?}"
        );
        let mut history = History::new();
        history.push_user("go on");
        history.push_assistant(interrupted_message(&redactor, "the key is tok-value-123456 done"));
        assert_eq!(history.len(), 2, "exactly one partial message was pushed");
        let stored = history.snapshot()[1]["content"].as_str().unwrap().to_string();
        assert!(!stored.contains("tok-value-123456"), "{stored}");
        assert!(stored.ends_with("\n\n[reply interrupted]"), "{stored}");
        let wire = history.messages_with("sys", &mut Resolver::none(), &TrimPlan::none());
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[2]["content"], json!(stored), "the next request contains the partial reply");
    }

    /// The `Debug` text names the variant overlay's keys but never its values.
    #[test]
    fn debug_shows_extra_keys_but_not_body_values() {
        let mut with_extra = settings_from_env(with_env(&[])).unwrap().unwrap();
        with_extra.extra =
            Some(serde_json::from_str(r#"{"reasoning":{"effort":"DUMMY-effort-marker-987654321"}}"#).unwrap());
        let debug = format!("{with_extra:?}");
        assert!(debug.contains("reasoning"), "the key names are shown: {debug}");
        assert!(!debug.contains("DUMMY-effort-marker-987654321"), "a body value leaked: {debug}");
        assert!(!debug.contains("sk-test-SECRETVALUE-0123456789"), "{debug}");

        let plain = settings_from_env(with_env(&[])).unwrap().unwrap();
        let debug = format!("{plain:?}");
        assert!(debug.contains("extra_keys"), "{debug}");
        assert!(!debug.contains("DUMMY"), "{debug}");
    }
}
