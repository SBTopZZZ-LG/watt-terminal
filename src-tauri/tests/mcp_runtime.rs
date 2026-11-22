//! The MCP runtime against real stdio servers: the dependency-free node fixture in tests/fixtures/mcp/server.cjs. Every
//! test that needs a server skips (prints why and returns) when `node` is not on PATH, so the docker image still runs.
//! Waits poll the manager's status instead of sleeping for a fixed time.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use watt_terminal_lib::core::external_tools::{ExternalApproval, ExternalToolHost};
use watt_terminal_lib::core::llm::TurnCancel;
use watt_terminal_lib::core::mcp::config::{McpLevel, McpServerConfig, McpSettings, McpTransport};
use watt_terminal_lib::core::mcp::logs::LOG_LINE_CHARS;
use watt_terminal_lib::core::mcp::manager::{McpManager, ServerState};
use watt_terminal_lib::core::mcp::secrets::SecretValues;

/// Generous: a cold node start on a loaded CI machine can take seconds.
const WAIT: Duration = Duration::from_secs(90);
const FIXTURE_SECRET_VALUE: &str = "fixture-secret-value-42";

fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/server.cjs").to_string_lossy().into_owned()
}

fn node_available() -> bool {
    Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Returns from the test (after saying so) when node is not installed.
macro_rules! require_node {
    () => {
        if !node_available() {
            eprintln!("SKIPPED: `node` is not on PATH; the MCP fixture tests did not run");
            return;
        }
    };
}

/// A stdio server running the fixture with `extra` arguments. No env values unless the test adds them.
fn fixture_server(name: &str, extra: &[&str]) -> McpServerConfig {
    let mut args = vec![fixture()];
    args.extend(extra.iter().map(|s| s.to_string()));
    McpServerConfig {
        name: name.into(),
        enabled: true,
        approval: McpLevel::Auto,
        tools: BTreeMap::new(),
        timeout_secs: None,
        transport: McpTransport::Stdio { command: "node".into(), args, cwd: None, env: vec![] },
    }
}

fn settings(servers: Vec<McpServerConfig>) -> McpSettings {
    McpSettings { servers }
}

/// Polls the status of `server` until `done` holds for its state. Panics with the last state when the wait runs out.
fn wait_for(manager: &McpManager, server: &str, done: impl Fn(&ServerState) -> bool) -> ServerState {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = manager.status().into_iter().find(|s| s.name == server).map(|s| s.state);
        if let Some(state) = state.as_ref().filter(|s| done(s)) {
            return state.clone();
        }
        assert!(Instant::now() < deadline, "timed out waiting on {server}; last state {state:?}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_connected(manager: &McpManager, server: &str) {
    wait_for(manager, server, |s| matches!(s, ServerState::Connected { .. }));
}

/// Calls a tool through the host interface, the way the agent does, with a turn that is never cancelled.
fn call(manager: &McpManager, tool: &str, args: Value) -> Result<String, String> {
    ExternalToolHost::call(manager, tool, args, &TurnCancel::new())
}

/// The pid the fixture reports for itself.
fn pid_of(manager: &McpManager, server: &str) -> u32 {
    let text = call(manager, &format!("mcp__{server}__pid"), json!({})).expect("pid call");
    text.trim().parse().expect("pid is a number")
}

/// Whether a process with this pid is running. Windows: tasklist. Unix: kill -0.
fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!(" {pid} ")))
    } else {
        Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success())
    }
}

/// Polls until the process is gone (kills are asynchronous). Panics when it is still running at the deadline.
fn wait_until_gone(pid: u32, what: &str) {
    let deadline = Instant::now() + WAIT;
    while process_alive(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid}) is still running");
        thread::sleep(Duration::from_millis(100));
    }
}

fn names_of(manager: &McpManager) -> Vec<String> {
    manager.schemas().into_iter().map(|s| s.name).collect()
}

#[test]
fn an_empty_manager_offers_nothing_and_refuses_unknown_names() {
    let manager = McpManager::start().expect("start");
    assert!(manager.schemas().is_empty());
    assert!(!manager.owns("mcp__fixture__echo"));
    assert_eq!(manager.approval("mcp__fixture__echo"), ExternalApproval::Never);
    let err = call(&manager, "mcp__fixture__echo", json!({})).unwrap_err();
    assert!(err.contains("unknown tool"), "{err}");
    manager.shutdown();
    manager.shutdown();
    assert!(manager.status().is_empty(), "a stopped manager answers with nothing");
}

#[test]
fn connect_list_and_call_a_stdio_server() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    assert_eq!(wait_for(&manager, "fixture", |s| matches!(s, ServerState::Connected { .. })), ServerState::Connected { tools: 10 });

    let names = names_of(&manager);
    assert!(names.contains(&"mcp__fixture__echo".to_string()), "{names:?}");
    // A name with spaces and unicode is offered under the sanitized, prefixed name.
    assert!(names.contains(&"mcp__fixture__weird_name__".to_string()), "{names:?}");

    let weird = manager.schemas().into_iter().find(|s| s.name == "mcp__fixture__weird_name__").unwrap();
    assert_eq!(weird.description, "[fixture] A tool whose name has spaces and unicode, with a $defs schema.");
    assert_eq!(weird.parameters["type"], "object");
    assert!(weird.parameters["$defs"]["Item"].is_object(), "{}", weird.parameters);

    assert!(manager.owns("mcp__fixture__echo"));
    assert_eq!(manager.approval("mcp__fixture__echo"), ExternalApproval::Auto);
    let shown = manager.display("mcp__fixture__echo", &json!({}));
    assert_eq!((shown.server.as_str(), shown.tool.as_str()), ("fixture", "echo"));

    assert_eq!(call(&manager, "mcp__fixture__echo", json!({"message": "hi"})), Ok("echo: hi".to_string()));
    assert!(call(&manager, "mcp__fixture__weird_name__", json!({"item": {"id": 3}})).is_ok());
    // Arguments must be an object (null means none).
    let err = call(&manager, "mcp__fixture__echo", json!([1, 2])).unwrap_err();
    assert!(err.contains("JSON object"), "{err}");
    assert_eq!(call(&manager, "mcp__fixture__echo", Value::Null), Ok("echo: undefined".to_string()));
    manager.shutdown();
}

#[test]
fn a_list_change_from_the_server_refreshes_the_offered_tools() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");
    assert!(!names_of(&manager).contains(&"mcp__fixture__added".to_string()));

    assert_eq!(call(&manager, "mcp__fixture__add-tool", json!({})), Ok("added".to_string()));
    let deadline = Instant::now() + WAIT;
    while !names_of(&manager).contains(&"mcp__fixture__added".to_string()) {
        assert!(Instant::now() < deadline, "the new tool was not offered: {:?}", names_of(&manager));
        thread::sleep(Duration::from_millis(50));
    }
    manager.shutdown();
}

#[test]
fn a_tool_set_to_never_is_hidden_from_the_model_and_refused() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let mut server = fixture_server("fixture", &[]);
    server.tools.insert("echo".into(), McpLevel::Never);
    manager.apply(&settings(vec![server]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");

    let names = names_of(&manager);
    assert!(!names.contains(&"mcp__fixture__echo".to_string()), "{names:?}");
    assert!(names.contains(&"mcp__fixture__pid".to_string()), "{names:?}");
    assert!(manager.owns("mcp__fixture__echo"), "a disabled tool is still owned, so the gate refuses it as Never");
    assert_eq!(manager.approval("mcp__fixture__echo"), ExternalApproval::Never);
    manager.shutdown();
}

#[test]
fn a_call_that_outlives_the_timeout_fails_with_the_timeout() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let mut server = fixture_server("fixture", &[]);
    server.timeout_secs = Some(1);
    manager.apply(&settings(vec![server]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");

    let started = Instant::now();
    let err = call(&manager, "mcp__fixture__slow", json!({"ms": 20000})).unwrap_err();
    assert!(err.contains("timed out after 1 s"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(20), "returned after {:?}", started.elapsed());
    // The timed-out call leaves the client usable.
    assert_eq!(call(&manager, "mcp__fixture__echo", json!({"message": "after"})), Ok("echo: after".to_string()));
    manager.shutdown();
}

#[test]
fn a_cancelled_turn_stops_its_call_and_the_client_stays_usable() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");

    let cancel = Arc::new(TurnCancel::new());
    let canceller = {
        let cancel = Arc::clone(&cancel);
        // Cancels a call that is already in flight (the slow answer is 20 s away).
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let outcome = ExternalToolHost::call(&manager, "mcp__fixture__slow", json!({"ms": 20000}), &cancel);
    canceller.join().unwrap();
    assert_eq!(outcome, Err("cancelled".to_string()));
    assert!(started.elapsed() < Duration::from_secs(15), "returned after {:?}", started.elapsed());
    assert_eq!(call(&manager, "mcp__fixture__echo", json!({"message": "still here"})), Ok("echo: still here".to_string()));
    manager.shutdown();
}

#[test]
fn a_tool_error_and_the_result_shapes_reach_the_model_as_text() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");

    assert_eq!(call(&manager, "mcp__fixture__fail", json!({})), Err("boom from fixture".to_string()));
    assert_eq!(call(&manager, "mcp__fixture__image", json!({})), Ok("[image: image/png, 3 bytes omitted]".to_string()));
    let big = call(&manager, "mcp__fixture__big", json!({})).unwrap();
    assert!(big.len() <= 32 * 1024, "{}", big.len());
    assert!(big.contains("[truncated "), "the cut is announced");
    manager.shutdown();
}

#[test]
fn a_server_that_fails_to_start_is_failed_with_its_error() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let server = McpServerConfig {
        name: "broken".into(),
        enabled: true,
        approval: McpLevel::Ask,
        tools: BTreeMap::new(),
        timeout_secs: None,
        transport: McpTransport::Stdio {
            command: "definitely-not-a-real-command-watt-test".into(),
            args: vec![],
            cwd: None,
            env: vec![],
        },
    };
    manager.apply(&settings(vec![server]), &BTreeMap::new()).expect("apply");
    let state = wait_for(&manager, "broken", |s| matches!(s, ServerState::Failed { .. }));
    let ServerState::Failed { error } = state else { unreachable!() };
    assert!(error.contains("could not start"), "{error}");
    manager.shutdown();
}

#[test]
fn a_call_to_a_server_that_lost_its_connection_says_why() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");

    // The fixture exits without answering: the connection is lost during the call.
    assert!(call(&manager, "mcp__fixture__exit", json!({})).is_err());
    wait_for(&manager, "fixture", |s| matches!(s, ServerState::Failed { .. }));
    // Within the reconnect cooldown the refusal names the failure and does not rebuild.
    let err = call(&manager, "mcp__fixture__echo", json!({"message": "x"})).unwrap_err();
    assert!(err.contains("is not connected (failed:"), "{err}");
    manager.shutdown();
}

#[test]
fn a_process_that_exits_while_connected_is_marked_failed_without_a_call() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager
        .apply(&settings(vec![fixture_server("fixture", &["exit-after-tools"])]), &BTreeMap::new())
        .expect("apply");
    let state = wait_for(&manager, "fixture", |s| matches!(s, ServerState::Failed { .. }));
    assert_eq!(state, ServerState::Failed { error: "server exited".to_string() });
    manager.shutdown();
}

#[test]
fn apply_restarts_only_what_changed_and_closes_what_was_removed() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let first = settings(vec![
        fixture_server("alpha", &[]),
        fixture_server("beta", &[]),
        fixture_server("gamma", &[]),
    ]);
    manager.apply(&first, &BTreeMap::new()).expect("apply");
    for name in ["alpha", "beta", "gamma"] {
        wait_connected(&manager, name);
    }
    let alpha_pid = pid_of(&manager, "alpha");
    let beta_pid = pid_of(&manager, "beta");
    let gamma_pid = pid_of(&manager, "gamma");

    // alpha: approval changed only (no restart). beta: its arguments changed (restart). gamma: removed.
    let mut alpha = fixture_server("alpha", &[]);
    alpha.approval = McpLevel::Ask;
    let beta = fixture_server("beta", &["changed-args"]);
    manager.apply(&settings(vec![alpha, beta]), &BTreeMap::new()).expect("apply");

    assert_eq!(pid_of(&manager, "alpha"), alpha_pid, "an unchanged server is not restarted");
    assert_eq!(manager.approval("mcp__alpha__echo"), ExternalApproval::Ask, "the new approval is used");
    wait_connected(&manager, "beta");
    let beta_now = pid_of(&manager, "beta");
    assert_ne!(beta_now, beta_pid, "a changed server is restarted");
    assert!(manager.status().iter().all(|s| s.name != "gamma"), "a removed server is gone");
    wait_until_gone(gamma_pid, "the removed server");
    assert!(!process_alive(beta_pid), "the old beta process is gone after its restart");
    manager.shutdown();
}

#[test]
fn a_disabled_server_is_stopped_and_offers_nothing() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");
    let pid = pid_of(&manager, "fixture");

    let mut off = fixture_server("fixture", &[]);
    off.enabled = false;
    manager.apply(&settings(vec![off]), &BTreeMap::new()).expect("apply");
    assert_eq!(wait_for(&manager, "fixture", |s| *s == ServerState::Disabled), ServerState::Disabled);
    assert!(names_of(&manager).is_empty());
    assert_eq!(manager.approval("mcp__fixture__echo"), ExternalApproval::Never);
    wait_until_gone(pid, "the disabled server");
    manager.shutdown();
}

#[test]
fn secret_values_are_redacted_in_the_log_and_in_results() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let mut server = fixture_server("fixture", &[]);
    server.transport = McpTransport::Stdio {
        command: "node".into(),
        args: vec![fixture()],
        cwd: None,
        env: vec!["FIXTURE_SECRET".into()],
    };
    let secrets = BTreeMap::from([(
        "fixture".to_string(),
        SecretValues {
            env_values: BTreeMap::from([("FIXTURE_SECRET".to_string(), FIXTURE_SECRET_VALUE.to_string())]),
            header_values: BTreeMap::new(),
        },
    )]);
    manager.apply(&settings(vec![server]), &secrets).expect("apply");
    wait_connected(&manager, "fixture");

    let leak = call(&manager, "mcp__fixture__leak", json!({})).expect("leak call");
    assert_eq!(leak, "secret is [REDACTED]");

    // The start line is written to stderr before the handshake; it must be in the log, redacted.
    let deadline = Instant::now() + WAIT;
    let logs = loop {
        let lines = manager.logs("fixture").unwrap_or_default();
        if !lines.is_empty() || Instant::now() > deadline {
            break lines;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let joined = logs.join("\n");
    assert!(joined.contains("fixture started FIXTURE_SECRET=[REDACTED]"), "{joined}");
    assert!(!joined.contains(FIXTURE_SECRET_VALUE), "{joined}");
    assert!(manager.logs("no-such-server").is_none());
    manager.shutdown();
}

#[test]
fn a_stderr_line_over_a_megabyte_is_cut_and_the_lines_after_it_are_still_logged() {
    require_node!();
    let manager = McpManager::start().expect("start");
    let noisy = fixture_server("noisy", &["long-stderr"]);
    manager.apply(&settings(vec![noisy]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "noisy");

    let deadline = Instant::now() + WAIT;
    let logs = loop {
        let lines = manager.logs("noisy").unwrap_or_default();
        if lines.iter().any(|l| l == "after the long line") || Instant::now() > deadline {
            break lines;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert!(logs.iter().any(|l| l == "after the long line"), "the line after the long one was not logged");
    let longest = logs.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    assert!(longest <= LOG_LINE_CHARS, "a kept line is {longest} characters");
    manager.shutdown();
}

#[test]
fn stopping_the_manager_kills_a_server_whose_tree_ignores_stdin_eof() {
    require_node!();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos();
    let pid_file = std::env::temp_dir().join(format!("watt-mcp-grandchild-{}-{nanos}.pid", std::process::id()));
    let _ = std::fs::remove_file(&pid_file);
    let manager = McpManager::start().expect("start");
    let stubborn = fixture_server("stubborn", &["stubborn", pid_file.to_str().expect("utf-8 temp path")]);
    manager.apply(&settings(vec![stubborn]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "stubborn");

    let deadline = Instant::now() + WAIT;
    let grandchild: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the fixture did not write its grandchild pid");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(process_alive(grandchild), "the grandchild runs before shutdown");

    manager.shutdown();
    wait_until_gone(grandchild, "the grandchild of a stubborn server");
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn a_dropped_manager_shuts_down_without_hanging() {
    require_node!();
    let manager = McpManager::start().expect("start");
    manager.apply(&settings(vec![fixture_server("fixture", &[])]), &BTreeMap::new()).expect("apply");
    wait_connected(&manager, "fixture");
    let pid = pid_of(&manager, "fixture");
    let started = Instant::now();
    drop(manager);
    assert!(started.elapsed() < Duration::from_secs(15), "drop took {:?}", started.elapsed());
    wait_until_gone(pid, "the server of a dropped manager");
}

/// Real server, not part of the normal run: needs `uvx` on PATH and network access to fetch the package.
/// Run with `cargo test --test mcp_runtime -- --ignored uvx_mcp_server_fetch_smoke`.
#[test]
#[ignore = "needs uvx and network access; run on demand"]
fn uvx_mcp_server_fetch_smoke() {
    let manager = McpManager::start().expect("start");
    let server = McpServerConfig {
        name: "fetch".into(),
        enabled: true,
        approval: McpLevel::Auto,
        tools: BTreeMap::new(),
        timeout_secs: None,
        transport: McpTransport::Stdio { command: "uvx".into(), args: vec!["mcp-server-fetch".into()], cwd: None, env: vec![] },
    };
    manager.apply(&settings(vec![server]), &BTreeMap::new()).expect("apply");
    let state = wait_for(&manager, "fetch", |s| !matches!(s, ServerState::Connecting));
    assert!(matches!(state, ServerState::Connected { .. }), "{state:?}");
    assert!(names_of(&manager).contains(&"mcp__fetch__fetch".to_string()), "{:?}", names_of(&manager));
    manager.shutdown();
}
