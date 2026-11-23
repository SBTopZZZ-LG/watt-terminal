//! The MCP settings as the app uses them: the saved servers, their secret values, and the running servers. Nothing here
//! touches Tauri; lib.rs has the thin command wrappers.
//!
//! Start: at launch, in the background, when a saved server is enabled (`warm`). Otherwise the runtime starts on the first
//! MCP command or model turn (`running`). A change applies at once. A server that has no names declared never reads the
//! secret store. `status` never starts the runtime and never reads the secret store.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::core::external_tools::ExternalApproval;
use crate::core::llm::TurnCancel;
use crate::core::settings::secrets::{SecretMap, Secrets};
use crate::core::settings::service::SettingsService;

use super::approval;
use super::config::{McpLevel, McpServerConfig, McpSettings, McpTransport};
use super::manager::{McpManager, ServerState, ServerStatus};
use super::parse::{ParsedImport, parse_pasted};
use super::secrets::{McpSecrets, SecretValues, write_server};

/// How long a turn waits for the enabled servers that are still connecting.
const CONNECT_WAIT: Duration = Duration::from_secs(8);
const SETTLE_POLL: Duration = Duration::from_millis(100);
/// Tool descriptions are shown up to this many characters.
const DESCRIPTION_CHARS: usize = 200;
const NO_NAMES: &[String] = &[];

/// Every configured server with its live state. Holds no secret value.
#[derive(Debug, Clone, Default, Serialize)]
pub struct McpView {
    pub servers: Vec<McpServerView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerView {
    pub config: McpServerConfig,
    pub state: McpStateView,
    pub tools: Vec<McpToolView>,
    pub secrets_present: McpSecretsPresent,
}

/// A server's state. `kind` is the wire tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpStateView {
    Disabled,
    Connecting,
    Connected { tools: usize },
    Failed { message: String },
    NeedsAuth { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolView {
    /// The server's own name for the tool.
    pub name: String,
    pub description: String,
    /// The tool's own level, or None when it inherits the server's.
    #[serde(rename = "override")]
    pub level_override: Option<McpLevel>,
    /// The level the model's calls to the tool are gated at (see `approval::resolve`).
    pub effective: McpLevel,
}

/// The names of the secrets stored for a server. Never their values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct McpSecretsPresent {
    pub env: Vec<String>,
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpImportResult {
    pub view: McpView,
    /// The names of the servers added.
    pub added: Vec<String>,
    pub warnings: Vec<String>,
}

/// What the chat composer shows (see `McpService::status`). Holds no URL, argument, env or header value, and no secret name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct McpStatus {
    pub servers: Vec<McpServerStatus>,
    /// Some enabled server is connecting.
    pub starting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServerStatus {
    /// The server's name, which is unique.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub state: McpStateView,
    /// The tools the server listed when it connected. 0 in any other state.
    pub tools: usize,
}

/// One server as the settings UI saves it. `config` holds names only; `env_values` and `header_values` are keyed by the
/// names the config declares. Holds secrets, so no `Debug`.
#[derive(Deserialize)]
pub struct McpServerInput {
    pub config: McpServerConfig,
    #[serde(default)]
    pub env_values: BTreeMap<String, String>,
    #[serde(default)]
    pub header_values: BTreeMap<String, String>,
}

/// What a model turn gets from MCP. `secret_values` are the values the turn's output must mask. Holds secrets: no `Debug`.
pub struct TurnMcp {
    pub manager: Arc<McpManager>,
    pub secret_values: Vec<String>,
}

impl TurnMcp {
    /// Gives the enabled servers that are still connecting up to `CONNECT_WAIT` to finish, so this turn offers their tools.
    /// Returns at once when none is connecting, or when the turn is cancelled. A server still connecting after that is not
    /// offered this turn.
    pub fn settle(&self, cancel: &TurnCancel) {
        self.settle_within(cancel, CONNECT_WAIT);
    }

    fn settle_within(&self, cancel: &TurnCancel, wait: Duration) {
        let deadline = Instant::now() + wait;
        while !cancel.is_cancelled() && Instant::now() < deadline {
            if !self.manager.status().iter().any(|s| s.state == ServerState::Connecting) {
                return;
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }
}

/// The runtime handle and the settings it was last applied with.
#[derive(Default)]
struct Started {
    manager: Option<Arc<McpManager>>,
    applied: McpSettings,
    /// How many times the runtime was applied. Tests read it to see that unchanged settings are not applied again.
    #[cfg(test)]
    applies: usize,
}

pub struct McpService {
    settings: Arc<SettingsService>,
    secrets: Arc<Secrets>,
    started: Mutex<Started>,
}

impl McpService {
    pub fn new(settings: Arc<SettingsService>) -> Self {
        let secrets = settings.secrets();
        Self { settings, secrets, started: Mutex::new(Started::default()) }
    }

    /// The servers with their live state. An empty view, with nothing started, when there are no servers or when
    /// settings.json cannot be used.
    pub fn view(&self) -> Result<McpView, String> {
        let Ok(settings) = self.settings.mcp_settings() else {
            return Ok(McpView::default());
        };
        // Nothing configured and nothing running: nothing to show, and nothing to start.
        if settings.servers.is_empty() && lock(&self.started).manager.is_none() {
            return Ok(McpView::default());
        }
        let manager = self.running(&settings)?;
        let status = manager.status();
        let servers = settings
            .servers
            .into_iter()
            .map(|config| {
                let live = status.iter().find(|s| s.name == config.name);
                Ok(McpServerView {
                    state: state_view(config.enabled, live),
                    tools: tools_view(&config, live),
                    secrets_present: self.secrets_present(&config)?,
                    config,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(McpView { servers })
    }

    /// Adds the servers of a pasted list (see parse.rs). Refused as a whole, naming the servers, when any name already exists.
    pub fn import(&self, text: &str) -> Result<McpImportResult, String> {
        let ParsedImport { servers, secrets, warnings } = parse_pasted(text).map_err(|problems| problems.join("; "))?;
        let added: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();
        let view = self.change(
            |settings| {
                let taken: Vec<&str> = settings
                    .servers
                    .iter()
                    .map(|s| s.name.as_str())
                    .filter(|name| added.iter().any(|a| a.as_str() == *name))
                    .collect();
                if !taken.is_empty() {
                    return Err(format!("these servers already exist, so nothing was imported: {}", taken.join(", ")));
                }
                settings.servers.extend(servers);
                Ok(())
            },
            |map| {
                for (name, values) in &secrets {
                    write_server(map, name, values);
                }
            },
        )?;
        Ok(McpImportResult { view, added, warnings })
    }

    /// Adds a server, or replaces the one with its name (the name cannot change). For each declared name, an absent value
    /// keeps the stored one, "" removes it, anything else replaces it. A value for an undeclared name is refused. A stored
    /// value whose name is no longer declared is removed.
    pub fn save_server(&self, input: McpServerInput) -> Result<McpView, String> {
        let McpServerInput { config, env_values, header_values } = input;
        let (env_names, header_names) = declared_names(&config.transport);
        let undeclared: Vec<&str> = env_values
            .keys()
            .filter(|name| !env_names.contains(*name))
            .chain(header_values.keys().filter(|name| !header_names.contains(*name)))
            .map(String::as_str)
            .collect();
        if !undeclared.is_empty() {
            return Err(format!("values were sent for names the server does not declare: {}", undeclared.join(", ")));
        }
        let stored = McpSecrets::new(&self.secrets).get(&config.name).map_err(|e| e.to_string())?;
        let kept = SecretValues {
            env_values: keep_or_replace(env_names, &env_values, &stored.env_values),
            header_values: keep_or_replace(header_names, &header_values, &stored.header_values),
        };
        let name = config.name.clone();
        self.change(
            |settings| {
                match settings.servers.iter_mut().find(|s| s.name == config.name) {
                    Some(slot) => *slot = config,
                    None => settings.servers.push(config),
                }
                Ok(())
            },
            |map| write_server(map, &name, &kept),
        )
    }

    /// Removes a server and its stored values.
    pub fn remove_server(&self, name: &str) -> Result<McpView, String> {
        self.change(
            |settings| {
                let before = settings.servers.len();
                settings.servers.retain(|s| s.name != name);
                if settings.servers.len() == before { Err(no_server(name)) } else { Ok(()) }
            },
            |map| write_server(map, name, &SecretValues::default()),
        )
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<McpView, String> {
        self.edit_server(name, |server| server.enabled = enabled)
    }

    pub fn set_server_approval(&self, name: &str, level: McpLevel) -> Result<McpView, String> {
        self.edit_server(name, |server| server.approval = level)
    }

    /// Sets one tool's level. `None` clears the override, so the tool inherits the server's level.
    pub fn set_tool_approval(&self, name: &str, tool: &str, level: Option<McpLevel>) -> Result<McpView, String> {
        self.edit_server(name, |server| match level {
            Some(level) => {
                server.tools.insert(tool.to_string(), level);
            }
            None => {
                server.tools.remove(tool);
            }
        })
    }

    /// Stops and starts one server now, then returns the view.
    pub fn restart(&self, name: &str) -> Result<McpView, String> {
        self.running_saved()?.restart(name)?;
        self.view()
    }

    /// The last log lines of a server, oldest first (the runtime redacts them).
    pub fn logs(&self, name: &str) -> Result<Vec<String>, String> {
        self.running_saved()?.logs(name).ok_or_else(|| no_server(name))
    }

    /// What a model turn needs: the runtime with the saved settings applied, and the MCP secret values to mask.
    /// The secret store is read only when a saved server declares an env or header name: without one nothing can be stored.
    pub fn for_turn(&self) -> Result<TurnMcp, String> {
        let settings = self.settings.mcp_settings()?;
        let manager = self.running(&settings)?;
        let declares_names = settings.servers.iter().any(|server| {
            let (env, headers) = declared_names(&server.transport);
            !env.is_empty() || !headers.is_empty()
        });
        let secret_values = if declares_names {
            McpSecrets::new(&self.secrets).all_values().map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };
        Ok(TurnMcp { manager, secret_values })
    }

    /// Starts the enabled servers at launch, so they connect before the first turn. Does nothing when no saved server is
    /// enabled. An error is dropped here: the view or a turn reports it when it matters.
    pub fn warm(&self) {
        let Ok(settings) = self.settings.mcp_settings() else {
            return;
        };
        if settings.servers.iter().any(|server| server.enabled) {
            let _ = self.running(&settings);
        }
    }

    /// The composer's view: each saved server's state, and whether an enabled one is connecting. It never starts the runtime
    /// and never reads the secret store, so it is cheap to poll. Settings that cannot be used show as no servers. Before the
    /// runtime starts, an enabled server is `Connecting`, since `warm` or the first use is about to start it.
    pub fn status(&self) -> McpStatus {
        let Ok(settings) = self.settings.mcp_settings() else {
            return McpStatus::default();
        };
        let manager = lock(&self.started).manager.clone();
        let live = manager.as_ref().map(|m| m.status()).unwrap_or_default();
        let servers: Vec<McpServerStatus> = settings
            .servers
            .into_iter()
            .map(|config| {
                let state = match (&manager, config.enabled) {
                    (None, true) => McpStateView::Connecting,
                    _ => state_view(config.enabled, live.iter().find(|s| s.name == config.name)),
                };
                let tools = match &state {
                    McpStateView::Connected { tools } => *tools,
                    _ => 0,
                };
                McpServerStatus { id: config.name.clone(), name: config.name, enabled: config.enabled, state, tools }
            })
            .collect();
        let starting = servers.iter().any(|s| s.enabled && s.state == McpStateView::Connecting);
        McpStatus { servers, starting }
    }

    /// Stops the servers and the runtime thread, if it was started. Safe to call twice.
    pub fn shutdown(&self) {
        let manager = lock(&self.started).manager.clone();
        if let Some(manager) = manager {
            manager.shutdown();
        }
    }

    fn edit_server(&self, name: &str, edit: impl FnOnce(&mut McpServerConfig)) -> Result<McpView, String> {
        self.change(
            |settings| {
                let server = settings.servers.iter_mut().find(|s| s.name == name).ok_or_else(|| no_server(name))?;
                edit(server);
                Ok(())
            },
            |_| {},
        )
    }

    /// Saves a change (settings and secrets in one save), then applies the saved servers and returns the view. A refused
    /// change is returned as it is. A change that is saved but not applied stays saved, and the error says so.
    fn change(
        &self,
        edit: impl FnOnce(&mut McpSettings) -> Result<(), String>,
        secrets: impl FnOnce(&mut SecretMap),
    ) -> Result<McpView, String> {
        self.settings.change_settings_with_secrets(|settings| edit(&mut settings.mcp), secrets)?;
        self.apply_saved()
            .map_err(|e| format!("the change was saved, but the servers were not updated: {e}"))?;
        self.view()
    }

    /// Applies the saved settings now, even if the runtime already runs them. Used after a change.
    fn apply_saved(&self) -> Result<(), String> {
        let settings = self.settings.mcp_settings()?;
        let mut started = lock(&self.started);
        self.apply_settings(&mut started, &settings).map(|_| ())
    }

    /// The saved settings, applied to the runtime (see `running`).
    fn running_saved(&self) -> Result<Arc<McpManager>, String> {
        let settings = self.settings.mcp_settings()?;
        self.running(&settings)
    }

    /// The runtime with `settings` applied. Starts it on first use, and applies again when `settings` differ from the ones
    /// it last ran (a hand edit of settings.json, for one). Changes nothing when they match.
    fn running(&self, settings: &McpSettings) -> Result<Arc<McpManager>, String> {
        let mut started = lock(&self.started);
        if let Some(manager) = &started.manager
            && started.applied == *settings
        {
            return Ok(manager.clone());
        }
        self.apply_settings(&mut started, settings)
    }

    /// Applies `settings` to the runtime, starting the runtime on first use. Records them as applied once they are.
    fn apply_settings(&self, started: &mut Started, settings: &McpSettings) -> Result<Arc<McpManager>, String> {
        let secrets = self.secret_map(settings)?;
        let manager = match &started.manager {
            Some(manager) => manager.clone(),
            None => Arc::new(McpManager::start()?),
        };
        started.manager = Some(manager.clone());
        manager.apply(settings, &secrets)?;
        started.applied = settings.clone();
        #[cfg(test)]
        {
            started.applies += 1;
        }
        Ok(manager)
    }

    /// The stored values of every server that declares a name. A server with no names is not read, so the store is not
    /// touched for it.
    fn secret_map(&self, settings: &McpSettings) -> Result<BTreeMap<String, SecretValues>, String> {
        let store = McpSecrets::new(&self.secrets);
        let mut map = BTreeMap::new();
        for server in &settings.servers {
            let (env, headers) = declared_names(&server.transport);
            if env.is_empty() && headers.is_empty() {
                continue;
            }
            let values = store.get(&server.name).map_err(|e| e.to_string())?;
            map.insert(server.name.clone(), values);
        }
        Ok(map)
    }

    /// The names of the values stored for a server. Never the values.
    fn secrets_present(&self, config: &McpServerConfig) -> Result<McpSecretsPresent, String> {
        let (env, headers) = declared_names(&config.transport);
        if env.is_empty() && headers.is_empty() {
            return Ok(McpSecretsPresent::default());
        }
        let stored = McpSecrets::new(&self.secrets).get(&config.name).map_err(|e| e.to_string())?;
        Ok(McpSecretsPresent {
            env: stored.env_values.keys().cloned().collect(),
            headers: stored.header_values.keys().cloned().collect(),
        })
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn no_server(name: &str) -> String {
    format!("no MCP server named '{name}'")
}

/// The secret names a server declares: its env variable names (stdio) or its header names (http).
fn declared_names(transport: &McpTransport) -> (&[String], &[String]) {
    match transport {
        McpTransport::Stdio { env, .. } => (env.as_slice(), NO_NAMES),
        McpTransport::Http { headers, .. } => (NO_NAMES, headers.as_slice()),
    }
}

/// The values to store for one kind of secret: a declared name takes the sent value, or else its stored one. An empty
/// sent value is kept here and dropped by `write_server`, which is how a value is removed.
fn keep_or_replace(
    declared: &[String],
    sent: &BTreeMap<String, String>,
    stored: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    declared
        .iter()
        .filter_map(|name| sent.get(name).or_else(|| stored.get(name)).map(|value| (name.clone(), value.clone())))
        .collect()
}

fn state_view(enabled: bool, live: Option<&ServerStatus>) -> McpStateView {
    match live.map(|s| &s.state) {
        Some(ServerState::Disabled) => McpStateView::Disabled,
        Some(ServerState::Connecting) => McpStateView::Connecting,
        Some(ServerState::Connected { tools }) => McpStateView::Connected { tools: *tools },
        Some(ServerState::Failed { error }) => McpStateView::Failed { message: error.clone() },
        Some(ServerState::NeedsAuth { message }) => McpStateView::NeedsAuth { message: message.clone() },
        None if enabled => McpStateView::Failed { message: "not started".into() },
        None => McpStateView::Disabled,
    }
}

/// The tools the server last listed, with their levels. Empty until it has listed them.
fn tools_view(config: &McpServerConfig, live: Option<&ServerStatus>) -> Vec<McpToolView> {
    let Some(live) = live else {
        return Vec::new();
    };
    live.tools
        .iter()
        .map(|tool| {
            let level_override = config.tools.get(&tool.original).copied();
            McpToolView {
                name: tool.original.clone(),
                description: tool.description.chars().take(DESCRIPTION_CHARS).collect(),
                level_override,
                effective: effective_level(config.enabled, config.approval, level_override),
            }
        })
        .collect()
}

fn effective_level(enabled: bool, server: McpLevel, tool: Option<McpLevel>) -> McpLevel {
    match approval::resolve(enabled, server, tool) {
        ExternalApproval::Ask => McpLevel::Ask,
        ExternalApproval::Auto => McpLevel::Auto,
        ExternalApproval::Never => McpLevel::Never,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::Command;

    use serde_json::json;

    use super::*;
    use crate::core::external_tools::ExternalToolHost;
    use crate::core::settings::secrets::MemoryStore;
    use crate::core::settings::settings_path;
    use crate::core::settings::test_support::TempDir;

    const TOKEN: &str = "tok-secret-value-1";
    const HEADER: &str = "Bearer hdr-secret-value-2";
    /// A command that does not exist: the server fails to start at once, with no process.
    const MISSING: &str = "watt-no-such-command";

    struct Fixture {
        dir: TempDir,
        store: MemoryStore,
        mcp: McpService,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new();
        let store = MemoryStore::default();
        let mcp = service_on(&dir, &store);
        Fixture { dir, store, mcp }
    }

    fn service_on(dir: &TempDir, store: &MemoryStore) -> McpService {
        let secrets = Arc::new(Secrets::new(Box::new(store.clone())));
        McpService::new(Arc::new(SettingsService::new(dir.path(), secrets)))
    }

    fn paste(name: &str) -> String {
        json!({"mcpServers": {name: {"command": MISSING, "env": {"TOKEN": TOKEN}}}}).to_string()
    }

    fn names(view: &McpView) -> Vec<String> {
        view.servers.iter().map(|s| s.config.name.clone()).collect()
    }

    fn pairs(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn stdio(name: &str, env: &[&str]) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            enabled: true,
            approval: McpLevel::Ask,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Stdio {
                command: MISSING.into(),
                args: vec![],
                cwd: None,
                env: env.iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    fn http(name: &str, url: &str, headers: &[&str]) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            enabled: true,
            approval: McpLevel::Ask,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Http { url: url.into(), headers: headers.iter().map(|s| s.to_string()).collect() },
        }
    }

    fn input(config: McpServerConfig, env: &[(&str, &str)], headers: &[(&str, &str)]) -> McpServerInput {
        McpServerInput { config, env_values: pairs(env), header_values: pairs(headers) }
    }

    fn read_settings(dir: &TempDir) -> String {
        fs::read_to_string(settings_path(dir.path())).unwrap()
    }

    fn values_of(f: &Fixture, server: &str) -> SecretValues {
        McpSecrets::new(&f.mcp.secrets).get(server).unwrap()
    }

    /// A loopback port that is free right now. Nothing listens on it, so a connect is refused at once.
    fn closed_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn import_adds_the_servers_and_keeps_their_values_out_of_the_view_and_the_file() {
        let f = fixture();
        let result = f.mcp.import(&paste("fetch")).unwrap();
        assert_eq!(result.added, ["fetch"]);
        assert_eq!(names(&result.view), ["fetch"]);
        assert_eq!(result.view.servers[0].secrets_present.env, ["TOKEN"]);
        let shown = serde_json::to_string(&result.view).unwrap();
        assert!(!shown.contains(TOKEN), "a value reached the view: {shown}");
        assert!(f.store.stored().unwrap().contains(TOKEN), "the value is in the secret store");
        let file = read_settings(&f.dir);
        assert!(file.contains("fetch") && !file.contains(TOKEN), "{file}");
        f.mcp.shutdown();
    }

    #[test]
    fn import_refuses_a_name_that_exists_and_changes_nothing() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        let before = read_settings(&f.dir);
        let two = json!({"mcpServers": {
            "ctx": {"command": MISSING},
            "fetch": {"command": MISSING, "env": {"TOKEN": "other-value-3"}}
        }})
        .to_string();
        let err = f.mcp.import(&two).unwrap_err();
        assert!(err.contains("fetch") && !err.contains("ctx"), "{err}");
        assert_eq!(read_settings(&f.dir), before);
        assert_eq!(values_of(&f, "fetch").env_values["TOKEN"], TOKEN);
        f.mcp.shutdown();
    }

    #[test]
    fn import_returns_its_warnings_and_a_bad_paste_names_no_value() {
        let f = fixture();
        let text = json!({"mcpServers": {"fetch": {"command": MISSING, "env": {"TOKEN": TOKEN}}}, "unrelated": 1}).to_string();
        let result = f.mcp.import(&text).unwrap();
        assert!(result.warnings.iter().any(|w| w.contains("unrelated")), "{:?}", result.warnings);
        let full = paste("ctx");
        let err = f.mcp.import(&full[..full.len() - 2]).unwrap_err();
        assert!(!err.contains(TOKEN), "{err}");
        f.mcp.shutdown();
    }

    #[test]
    fn saving_a_server_keeps_replaces_or_removes_each_value_by_name() {
        let f = fixture();
        f.mcp.save_server(input(stdio("fetch", &["A", "B"]), &[("A", "a-value-1"), ("B", "b-value-1")], &[])).unwrap();
        // A is left out, so it keeps its value; B is "", so it is removed.
        f.mcp.save_server(input(stdio("fetch", &["A", "B"]), &[("B", "")], &[])).unwrap();
        let values = values_of(&f, "fetch");
        assert_eq!(values.env_values.get("A").map(String::as_str), Some("a-value-1"));
        assert!(!values.env_values.contains_key("B"));
        // A new value replaces the old one.
        f.mcp.save_server(input(stdio("fetch", &["A"]), &[("A", "a-value-2")], &[])).unwrap();
        assert_eq!(values_of(&f, "fetch").env_values["A"], "a-value-2");
        // A value for a name the config does not declare is refused and changes nothing.
        let err = f.mcp.save_server(input(stdio("fetch", &["A"]), &[("Z", "z-value-9")], &[])).unwrap_err();
        assert!(err.contains('Z') && !err.contains("z-value-9"), "{err}");
        assert_eq!(values_of(&f, "fetch").env_values["A"], "a-value-2");
        // Dropping the name from the config removes its stored value.
        f.mcp.save_server(input(stdio("fetch", &[]), &[], &[])).unwrap();
        assert!(values_of(&f, "fetch").env_values.is_empty());
        f.mcp.shutdown();
    }

    #[test]
    fn saving_an_http_server_keeps_its_header_values_by_name() {
        let f = fixture();
        let url = format!("http://127.0.0.1:{}/mcp", closed_port());
        f.mcp.save_server(input(http("ctx", &url, &["Authorization"]), &[], &[("Authorization", HEADER)])).unwrap();
        f.mcp.save_server(input(http("ctx", &url, &["Authorization"]), &[], &[])).unwrap();
        assert_eq!(values_of(&f, "ctx").header_values["Authorization"], HEADER);
        let view = f.mcp.view().unwrap();
        assert_eq!(view.servers[0].secrets_present.headers, ["Authorization"]);
        assert!(!serde_json::to_string(&view).unwrap().contains(HEADER));
        f.mcp.shutdown();
    }

    #[test]
    fn removing_a_server_clears_its_secrets_and_no_other_servers() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        f.mcp.import(&paste("ctx")).unwrap();
        let view = f.mcp.remove_server("fetch").unwrap();
        assert_eq!(names(&view), ["ctx"]);
        assert!(values_of(&f, "fetch").env_values.is_empty());
        assert_eq!(values_of(&f, "ctx").env_values.get("TOKEN").map(String::as_str), Some(TOKEN));
        assert!(f.mcp.remove_server("fetch").unwrap_err().contains("fetch"));
        f.mcp.shutdown();
    }

    #[test]
    fn enabled_and_approval_changes_persist_and_show_in_the_view() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        f.mcp.set_enabled("fetch", false).unwrap();
        f.mcp.set_server_approval("fetch", McpLevel::Auto).unwrap();
        let view = f.mcp.view().unwrap();
        let server = &view.servers[0];
        assert!(!server.config.enabled);
        assert_eq!(server.config.approval, McpLevel::Auto);
        assert_eq!(server.state, McpStateView::Disabled);
        // A fresh service on the same folder reads the same settings.
        let again = service_on(&f.dir, &f.store).view().unwrap();
        assert_eq!(again.servers[0].config, server.config);
        assert!(f.mcp.set_enabled("nope", true).unwrap_err().contains("nope"));
        f.mcp.shutdown();
    }

    #[test]
    fn a_server_with_no_names_does_not_read_the_secret_store() {
        let f = fixture();
        let text = json!({"mcpServers": {"plain": {"command": MISSING}}}).to_string();
        f.mcp.import(&text).unwrap();
        // A store that fails every read: a server that declares no names must not need it.
        let store = MemoryStore::default();
        store.set_fail_loads(true);
        let mcp = service_on(&f.dir, &store);
        let view = mcp.view().unwrap();
        assert_eq!(names(&view), ["plain"]);
        assert!(view.servers[0].secrets_present.env.is_empty());
        assert_eq!(store.loads(), 0, "the secret store was read for a server with no names");
        mcp.shutdown();
    }

    #[test]
    fn a_turn_masks_the_mcp_values() {
        let f = fixture();
        f.mcp.save_server(input(stdio("fetch", &["TOKEN"]), &[("TOKEN", TOKEN)], &[])).unwrap();
        let turn = f.mcp.for_turn().unwrap();
        assert!(turn.secret_values.iter().any(|v| v == TOKEN), "{:?}", turn.secret_values);
        f.mcp.shutdown();
    }

    /// A service on the same folder and store with no cached secrets, so a read shows in `loads`.
    fn cold(f: &Fixture) -> McpService {
        service_on(&f.dir, &f.store)
    }

    #[test]
    fn a_turn_reads_no_secret_when_no_server_declares_a_name() {
        let f = fixture();
        f.mcp.save_server(input(stdio("fetch", &[]), &[], &[])).unwrap();
        f.mcp.save_server(input(http("docs", "http://127.0.0.1:9/mcp", &[]), &[], &[])).unwrap();
        let mcp = cold(&f);
        let before = f.store.loads();
        let turn = mcp.for_turn().unwrap();
        assert!(turn.secret_values.is_empty(), "{:?}", turn.secret_values);
        assert_eq!(f.store.loads(), before, "a turn read the secret store with no names declared");
        mcp.shutdown();
    }

    #[test]
    fn a_turn_reads_the_secrets_when_a_server_declares_a_name() {
        let f = fixture();
        f.mcp.save_server(input(stdio("fetch", &["TOKEN"]), &[("TOKEN", TOKEN)], &[])).unwrap();
        let mcp = cold(&f);
        let before = f.store.loads();
        let turn = mcp.for_turn().unwrap();
        assert!(f.store.loads() > before, "a turn with a declared name did not read the store");
        assert!(turn.secret_values.iter().any(|v| v == TOKEN), "{:?}", turn.secret_values);
        mcp.shutdown();
    }

    #[test]
    fn a_turn_is_an_error_when_the_secret_read_fails_and_a_name_is_declared() {
        let f = fixture();
        let docs = http("docs", "http://127.0.0.1:9/mcp", &["Authorization"]);
        f.mcp.save_server(input(docs, &[], &[("Authorization", HEADER)])).unwrap();
        f.store.set_fail_loads(true);
        let mcp = cold(&f);
        let err = mcp.for_turn().err().expect("a failed read must not start a turn");
        assert!(!err.contains(HEADER), "a value reached the error: {err}");
        mcp.shutdown();
    }

    /// A loopback server that accepts connections and never answers, so its connect stays `Connecting` until a timeout.
    fn silent_server_url() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        (listener, url)
    }

    #[test]
    fn settle_returns_at_once_when_no_server_is_connecting() {
        let f = fixture();
        let cancel = TurnCancel::new();
        // No servers at all.
        let turn = f.mcp.for_turn().unwrap();
        let started = Instant::now();
        turn.settle_within(&cancel, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(1), "settle waited with no server");
        // A disabled server is never connecting.
        let mut config = stdio("fetch", &[]);
        config.enabled = false;
        f.mcp.save_server(input(config, &[], &[])).unwrap();
        let turn = f.mcp.for_turn().unwrap();
        let started = Instant::now();
        turn.settle_within(&cancel, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(1), "settle waited for a disabled server");
        f.mcp.shutdown();
    }

    #[test]
    fn settle_is_bounded_by_its_wait_for_a_server_that_stays_connecting() {
        let f = fixture();
        let (_listener, url) = silent_server_url();
        f.mcp.save_server(input(http("slow", &url, &[]), &[], &[])).unwrap();
        let turn = f.mcp.for_turn().unwrap();
        let started = Instant::now();
        turn.settle_within(&TurnCancel::new(), Duration::from_millis(400));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(400), "settle returned early: {waited:?}");
        assert!(waited < Duration::from_secs(10), "settle ignored its wait: {waited:?}");
        let states: Vec<ServerState> = turn.manager.status().into_iter().map(|s| s.state).collect();
        assert_eq!(states, [ServerState::Connecting], "the silent server should still be connecting");
        f.mcp.shutdown();
    }

    #[test]
    fn settle_stops_at_once_when_the_turn_is_cancelled() {
        let f = fixture();
        let (_listener, url) = silent_server_url();
        f.mcp.save_server(input(http("slow", &url, &[]), &[], &[])).unwrap();
        let turn = f.mcp.for_turn().unwrap();
        let cancel = TurnCancel::new();
        cancel.cancel();
        let started = Instant::now();
        turn.settle_within(&cancel, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(1), "settle waited after a cancel");
        f.mcp.shutdown();
    }

    #[test]
    fn a_hand_edit_is_applied_on_the_next_turn_and_view_without_a_mutation() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        let server_names = |manager: &McpManager| -> Vec<String> {
            let mut names: Vec<String> = manager.status().into_iter().map(|s| s.name).collect();
            names.sort();
            names
        };
        // Edit settings.json by hand, then reload it. No mcp_* command runs.
        hand_edit_servers(&f, vec![stdio("ctx", &[])]);
        let turn = f.mcp.for_turn().unwrap();
        assert_eq!(server_names(&turn.manager), ["ctx"]);
        hand_edit_servers(&f, vec![stdio("fetch", &[]), stdio("ctx", &[])]);
        let view = f.mcp.view().unwrap();
        assert_eq!(names(&view), ["fetch", "ctx"]);
        assert_eq!(server_names(&f.mcp.for_turn().unwrap().manager), ["ctx", "fetch"]);
        f.mcp.shutdown();
    }

    #[test]
    fn unchanged_settings_are_not_applied_again() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        let applies = |f: &Fixture| lock(&f.mcp.started).applies;
        let before = applies(&f);
        f.mcp.for_turn().unwrap();
        f.mcp.view().unwrap();
        f.mcp.view().unwrap();
        assert_eq!(applies(&f), before, "unchanged settings were applied again");
        hand_edit_servers(&f, vec![stdio("ctx", &[])]);
        f.mcp.for_turn().unwrap();
        assert_eq!(applies(&f), before + 1, "a changed file was not applied");
        f.mcp.shutdown();
    }

    /// Writes the MCP servers into settings.json by hand, then reloads the settings as the app does.
    fn hand_edit_servers(f: &Fixture, servers: Vec<McpServerConfig>) {
        let path = settings_path(f.dir.path());
        let mut file: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file["mcp"] = serde_json::to_value(McpSettings { servers }).unwrap();
        fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        f.mcp.settings.reload().unwrap();
    }

    #[test]
    fn a_broken_settings_file_shows_nothing_and_refuses_changes_and_is_left_alone() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let broken = b"{ not settings";
        fs::write(settings_path(dir.path()), broken).unwrap();
        let mcp = service_on(&dir, &MemoryStore::default());
        assert!(mcp.view().unwrap().servers.is_empty());
        assert!(mcp.import(&paste("fetch")).unwrap_err().contains("unchanged"));
        assert!(mcp.remove_server("fetch").unwrap_err().contains("unchanged"));
        assert!(mcp.logs("fetch").is_err());
        assert_eq!(fs::read(settings_path(dir.path())).unwrap(), broken, "the broken file was overwritten");
    }

    #[test]
    fn status_before_the_runtime_starts_shows_enabled_as_connecting_and_reads_no_secret() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        let mut off = stdio("ctx", &[]);
        off.enabled = false;
        f.mcp.save_server(input(off, &[], &[])).unwrap();
        // A store that fails every read: the status must not need it, and must not start the runtime.
        let store = MemoryStore::default();
        store.set_fail_loads(true);
        let mcp = service_on(&f.dir, &store);
        let status = mcp.status();
        assert!(lock(&mcp.started).manager.is_none(), "status started the runtime");
        assert_eq!(store.loads(), 0, "status read the secret store");
        assert_eq!(status.servers.len(), 2, "{status:?}");
        assert_eq!(status.servers[0].name, "fetch");
        assert_eq!(status.servers[0].id, "fetch");
        assert!(status.servers[0].enabled);
        assert_eq!(status.servers[0].state, McpStateView::Connecting);
        assert_eq!(status.servers[1].state, McpStateView::Disabled);
        assert_eq!(status.servers[1].tools, 0);
        assert!(status.starting);
        let shown = serde_json::to_string(&status).unwrap();
        assert!(!shown.contains("command") && !shown.contains("TOKEN") && !shown.contains(TOKEN), "{shown}");
        f.mcp.shutdown();
    }

    #[test]
    fn status_of_a_broken_settings_file_is_empty_and_not_an_error() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(settings_path(dir.path()), b"{ not settings").unwrap();
        let status = service_on(&dir, &MemoryStore::default()).status();
        assert!(status.servers.is_empty());
        assert!(!status.starting);
    }

    #[test]
    fn status_is_starting_while_an_enabled_server_connects() {
        let f = fixture();
        let (_listener, url) = silent_server_url();
        f.mcp.save_server(input(http("slow", &url, &[]), &[], &[])).unwrap();
        let status = f.mcp.status();
        assert_eq!(status.servers[0].state, McpStateView::Connecting, "{status:?}");
        assert!(status.starting);
        f.mcp.shutdown();
    }

    #[test]
    fn warm_starts_the_runtime_only_when_a_server_is_enabled() {
        let f = fixture();
        f.mcp.import(&paste("fetch")).unwrap();
        let mcp = cold(&f);
        mcp.warm();
        assert!(lock(&mcp.started).manager.is_some(), "warm did not start the runtime for an enabled server");
        mcp.shutdown();
        f.mcp.set_enabled("fetch", false).unwrap();
        let mcp = cold(&f);
        mcp.warm();
        assert!(lock(&mcp.started).manager.is_none(), "warm started the runtime with no server enabled");
        f.mcp.shutdown();
    }

    fn node_available() -> bool {
        Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn settle_returns_once_a_connecting_node_fixture_server_connects() {
        if !node_available() {
            eprintln!("SKIPPED: `node` is not on PATH; the settle fixture test did not run");
            return;
        }
        let f = fixture();
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/server.cjs");
        let mut config = stdio("fixture", &[]);
        config.transport = McpTransport::Stdio {
            command: "node".into(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env: vec![],
        };
        f.mcp.save_server(input(config, &[], &[])).unwrap();
        let turn = f.mcp.for_turn().unwrap();
        let started = Instant::now();
        turn.settle_within(&TurnCancel::new(), Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(60), "settle ran to its wait");
        let states: Vec<ServerState> = turn.manager.status().into_iter().map(|s| s.state).collect();
        assert!(matches!(states.as_slice(), [ServerState::Connected { .. }]), "{states:?}");
        f.mcp.shutdown();
    }

    #[test]
    fn a_node_fixture_server_connects_lists_its_tools_and_a_never_override_hides_one() {
        if !node_available() {
            eprintln!("SKIPPED: `node` is not on PATH; the MCP service fixture test did not run");
            return;
        }
        let f = fixture();
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/server.cjs");
        let config = McpServerConfig {
            name: "fixture".into(),
            enabled: true,
            approval: McpLevel::Auto,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Stdio {
                command: "node".into(),
                args: vec![script.to_string_lossy().into_owned()],
                cwd: None,
                env: vec![],
            },
        };
        f.mcp.save_server(McpServerInput { config, env_values: BTreeMap::new(), header_values: BTreeMap::new() }).unwrap();

        // Wait for the connect, polling the view.
        let deadline = Instant::now() + Duration::from_secs(90);
        let view = loop {
            let view = f.mcp.view().unwrap();
            let connected = matches!(view.servers[0].state, McpStateView::Connected { .. });
            if connected || Instant::now() > deadline {
                break view;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(matches!(view.servers[0].state, McpStateView::Connected { .. }), "{:?}", view.servers[0].state);
        let echo = view.servers[0].tools.iter().find(|t| t.name == "echo").expect("the echo tool is listed");
        assert_eq!(echo.effective, McpLevel::Auto);

        let view = f.mcp.set_tool_approval("fixture", "echo", Some(McpLevel::Never)).unwrap();
        let echo = view.servers[0].tools.iter().find(|t| t.name == "echo").unwrap();
        assert_eq!(echo.level_override, Some(McpLevel::Never));
        assert_eq!(echo.effective, McpLevel::Never);
        let offered: Vec<String> = f.mcp.for_turn().unwrap().manager.schemas().into_iter().map(|s| s.name).collect();
        assert!(!offered.iter().any(|n| n.ends_with("__echo")), "{offered:?}");
        assert!(offered.iter().any(|n| n.ends_with("__pid")), "{offered:?}");
        f.mcp.shutdown();
    }
}
