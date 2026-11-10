//! Tests for the chat agent loop (core/chat/agent_loop) against a local fake provider. No network and no keys: the
//! provider is a std TcpListener on 127.0.0.1 that answers each request with the next scripted reply and keeps the raw
//! request. The tools are a fake, so no shell is involved.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate};
use agent_term_lib::core::chat::agent_loop::{Conversation, LlmAgent, LlmSettings, MAX_COMMANDS_PER_TURN, MAX_MODEL_CALLS};
use agent_term_lib::core::chat::history::History;
use agent_term_lib::core::chat::prompt;
use agent_term_lib::core::chat::todo;
use agent_term_lib::core::llm::{TurnCancel, http_client};
use agent_term_lib::core::record::{Cwd, RecordSummary};
use agent_term_lib::core::tools::{AgentTools, CommandView, RunOutcome, ScreenView, WaitView};
use serde_json::{Value, json};

const KEY: &str = "sk-or-test-SECRETVALUE-0123456789";
const MODEL: &str = "deepseek/deepseek-v4.1-flash";

/// One scripted provider answer.
struct Script {
    status: &'static str,
    body: String,
    /// Sent as an SSE stream, in small pieces.
    sse: bool,
    /// Sends the head (and any body), then keeps the connection open without finishing it.
    stall: bool,
}

impl Script {
    fn json(body: Value) -> Self {
        Script { status: "200 OK", body: body.to_string(), sse: false, stall: false }
    }

    fn sse(body: &str) -> Self {
        Script { status: "200 OK", body: body.to_string(), sse: true, stall: false }
    }

    fn error(status: &'static str, body: Value) -> Self {
        Script { status, body: body.to_string(), sse: false, stall: false }
    }

    fn stall() -> Self {
        Script { status: "200 OK", body: String::new(), sse: true, stall: true }
    }
}

/// A fake provider: its chat URL and the raw requests it received, in order.
struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Provider {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serves one script per connection, in order. Once the scripts run out the listener closes.
fn serve(scripts: Vec<Script>) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let url = format!("http://{}/v1/chat/completions", listener.local_addr().expect("local addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    thread::spawn(move || {
        for script in scripts {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let _ = sock.set_nodelay(true);
            let raw = read_request(&mut sock);
            log.lock().unwrap().push(raw);
            write_response(&mut sock, &script);
            if script.stall {
                thread::sleep(Duration::from_secs(5));
            }
        }
    });
    Provider { url, requests }
}

fn write_response(sock: &mut TcpStream, script: &Script) {
    let framing = if script.sse || script.stall {
        "Connection: close\r\n".to_string()
    } else {
        format!("Content-Length: {}\r\nConnection: close\r\n", script.body.len())
    };
    let content_type = if script.sse { "text/event-stream" } else { "application/json" };
    let head = format!("HTTP/1.1 {}\r\nContent-Type: {content_type}\r\n{framing}\r\n", script.status);
    let _ = sock.write_all(head.as_bytes());
    if script.sse {
        // Small pieces, so the SSE framer sees lines split across network reads.
        for piece in script.body.as_bytes().chunks(5) {
            let _ = sock.write_all(piece);
        }
    } else {
        let _ = sock.write_all(script.body.as_bytes());
    }
    let _ = sock.flush();
}

/// Reads one HTTP request (head, then Content-Length bytes of body) and returns it as text.
fn read_request(sock: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        match sock.read(&mut buf) {
            Ok(0) | Err(_) => break data.len(),
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end.min(data.len())]).to_string();
    let content_length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < header_end + content_length {
        match sock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

/// The JSON body of a raw request.
fn body_of(raw: &str) -> Value {
    let (_, body) = raw.split_once("\r\n\r\n").expect("request has a body");
    serde_json::from_str(body).expect("request body is JSON")
}

/// The value of a request header (name matched without case), if present.
fn header_of(raw: &str, name: &str) -> Option<String> {
    let head = raw.split_once("\r\n\r\n").map_or(raw, |(h, _)| h);
    head.lines().skip(1).filter_map(|l| l.split_once(':')).find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string())
}

/// A non-streamed reply with the given tool calls, as (id, name, arguments).
fn tool_reply(calls: &[(&str, &str, &str)]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}}))
        .collect();
    json!({"choices": [{"index": 0, "finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": calls}}]})
}

/// A non-streamed plain answer.
fn text_reply(text: &str) -> Value {
    json!({"choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": text}}]})
}

/// A streamed answer, one delta per part.
fn sse_text(parts: &[&str]) -> String {
    let mut s = String::new();
    for part in parts {
        s.push_str(&format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {"content": part}}]})));
    }
    s.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
    s.push_str("data: [DONE]\n\n");
    s
}

/// A streamed tool call to `get_cwd` with the arguments split over two deltas, then the end of the reply.
const SSE_CWD_CALL: &str = "\
data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_s1\",\"type\":\"function\",\"function\":{\"name\":\"get_cwd\",\"arguments\":\"\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n\n";

/// The tools the fake agent may reach. Every call is recorded; nothing touches a shell.
#[derive(Default)]
struct FakeTools {
    reached: Mutex<Vec<String>>,
    /// When set, a `get_cwd` call cancels this token (a cancel that lands during a tool call).
    cancel_on_cwd: Option<Arc<TurnCancel>>,
    /// The output a `run_command` returns. Defaults to "hello".
    command_output: Option<String>,
}

impl FakeTools {
    fn reached(&self) -> Vec<String> {
        self.reached.lock().unwrap().clone()
    }
}

impl AgentTools for FakeTools {
    // The agent reaches every tool through `call`, which is overridden below, so these are never used.
    fn list_commands(&self, _n: usize) -> Vec<RecordSummary> {
        panic!("the loop calls tools by name through `call`")
    }
    fn get_command(&self, _id: u64, _offset: usize, _limit: usize) -> Result<CommandView, String> {
        panic!("the loop calls tools by name through `call`")
    }
    fn get_screen(&self) -> ScreenView {
        panic!("the loop calls tools by name through `call`")
    }
    fn get_cwd(&self) -> Option<Cwd> {
        panic!("the loop calls tools by name through `call`")
    }
    fn run_command(&self, _command: &str, _timeout: Duration) -> Result<RunOutcome, String> {
        panic!("the loop calls tools by name through `call`")
    }
    fn wait(&self, _seconds: u64, _ignore_updates: bool) -> Result<WaitView, String> {
        panic!("the loop calls tools by name through `call`")
    }
    fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
        panic!("the loop calls tools by name through `call`")
    }

    fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.reached.lock().unwrap().push(name.to_string());
        match name {
            "get_cwd" => {
                if let Some(cancel) = &self.cancel_on_cwd {
                    cancel.cancel();
                }
                Ok(json!({"host": "box", "path": "/home/u"}))
            }
            "wait" => Ok(json!({
                "reason": "timeout", "waited_ms": 5000, "state": "running",
                "screen": {"rows": ["sleep 30"], "cursor": [1, 0], "alternate_screen": false, "size": [24, 80],
                           "state": "running", "observer_status": "ok", "integration_status": "live"},
            })),
            "run_command" => {
                let output = self.command_output.clone().unwrap_or_else(|| "hello\n".to_string());
                Ok(json!({
                    "outcome": "finished",
                    "record": {"id": 7, "exit_code": 0, "status": "complete", "cwd": {"host": "box", "path": "/home/u"}, "output": output},
                }))
            }
            other => Ok(json!({"tool": other, "args": args})),
        }
    }
}

fn settings(url: &str, stream: bool) -> LlmSettings {
    LlmSettings {
        chat_url: url.to_string(),
        api_key: KEY.to_string(),
        model: MODEL.to_string(),
        headers: Vec::new(),
        stream,
        max_tokens: None,
        context_limit: None,
    }
}

fn agent_for(s: &LlmSettings, conv: &Conversation, cancel: Arc<TurnCancel>) -> LlmAgent {
    LlmAgent::new(&http_client().expect("client"), s.clone(), &conv.id(), conv.history(), conv.todos(), cancel)
}

/// Runs one turn and returns every event it emitted, `Done` included.
fn turn(agent: &mut LlmAgent, tools: &dyn AgentTools, gate: &ApprovalGate, text: &str) -> Vec<AgentEvent> {
    let log: Mutex<Vec<AgentEvent>> = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
    let ctx = AgentCtx::new(tools, gate, &emit);
    agent::run_turn(agent, text, &ctx);
    log.into_inner().unwrap()
}

/// The stored history without the system message.
fn stored(history: &Arc<Mutex<History>>) -> Vec<Value> {
    history.lock().unwrap().messages("")[1..].to_vec()
}

fn texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn deltas(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// (name, decision, ok) of every ToolResult, in order.
fn decisions(events: &[AgentEvent]) -> Vec<(String, &'static str, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { name, decision, ok, .. } => Some((name.clone(), *decision, *ok)),
            _ => None,
        })
        .collect()
}

/// True when a ToolResult event carries the raw output that includes the key (the UI's full view of the result).
fn tool_result_has_raw_output(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| matches!(e, AgentEvent::ToolResult { result, .. } if result.to_string().contains(KEY)))
}

/// The content of the tool message that answers `call_id`.
fn tool_content(messages: &[Value], call_id: &str) -> String {
    messages
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == call_id)
        .and_then(|m| m["content"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("no tool message for {call_id} in {messages:?}"))
}

/// Every assistant tool call has exactly one tool message after it, and no tool message answers nothing.
fn assert_paired(messages: &[Value]) {
    let mut calls: Vec<String> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        if m["role"] != "assistant" {
            continue;
        }
        for call in m["tool_calls"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let id = call["id"].as_str().unwrap().to_string();
            let answers = messages[i + 1..]
                .iter()
                .take_while(|n| n["role"] != "user" && n["role"] != "assistant")
                .filter(|n| n["tool_call_id"] == id.as_str())
                .count();
            assert_eq!(answers, 1, "call {id} needs exactly one result: {messages:?}");
            calls.push(id);
        }
    }
    for m in messages.iter().filter(|m| m["role"] == "tool") {
        let id = m["tool_call_id"].as_str().unwrap_or_default();
        assert!(calls.iter().any(|c| c == id), "tool message {id} answers no call");
    }
}

fn no_gate() -> ApprovalGate {
    ApprovalGate::with_timeout(Duration::from_millis(50))
}

#[test]
fn streamed_answer_emits_deltas_and_records_the_history() {
    let provider = serve(vec![Script::sse(&sse_text(&["Hel", "lo"]))]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, true), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "hi");

    assert_eq!(deltas(&events), ["Hel", "lo"]);
    assert!(texts(&events).is_empty(), "a streamed reply is not also sent as one Text");
    assert!(matches!(events.first(), Some(AgentEvent::Thinking { active: true })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Thinking { active: false })));
    assert!(matches!(events.last(), Some(AgentEvent::Done)));
    let msgs = stored(&conv.history());
    assert_eq!(msgs, vec![json!({"role": "user", "content": "hi"}), json!({"role": "assistant", "content": "Hello"})]);
    assert_eq!(provider.requests().len(), 1);
}

#[test]
fn non_streamed_answer_emits_one_text() {
    let provider = serve(vec![Script::json(text_reply("Hi there"))]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "hi");

    assert_eq!(texts(&events), ["Hi there"]);
    assert!(deltas(&events).is_empty());
    assert!(matches!(events.last(), Some(AgentEvent::Done)));
}

#[test]
fn read_tool_result_goes_back_to_the_model_under_the_same_call_id() {
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_1", "get_cwd", "{}")])),
        Script::json(text_reply("You are in box:/home/u")),
    ]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &tools, &no_gate(), "where am I");

    assert_eq!(decisions(&events), [("get_cwd".to_string(), "read", true)]);
    assert_eq!(tools.reached(), ["get_cwd"]);
    assert_eq!(texts(&events), ["You are in box:/home/u"]);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    let second = body_of(&requests[1]);
    let tool_msg = second["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap().clone();
    assert_eq!(tool_msg["tool_call_id"], "call_1");
    assert_eq!(tool_msg["content"], "box:/home/u", "the projected result, not the raw JSON");
    assert_paired(&stored(&conv.history()));
}

/// `wait` goes through the loop as a read tool: no approval card, its result under its own call id, projected as text.
#[test]
fn wait_is_a_read_tool_under_its_call_id() {
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_w", "wait", "{\"seconds\":5}")])),
        Script::json(text_reply("it is still running")),
    ]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &tools, &no_gate(), "wait a bit");

    assert_eq!(decisions(&events), [("wait".to_string(), "read", true)]);
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })), "wait asked for approval");
    assert_eq!(tools.reached(), ["wait"]);
    let msgs = stored(&conv.history());
    assert!(tool_content(&msgs, "call_w").starts_with("waited 5.0s: the wait time ran out"), "{msgs:?}");
    assert_eq!(texts(&events), ["it is still running"]);
    assert_paired(&msgs);
}

/// The per-turn command limit counts only `run_command`: waits in the same turn do not use it up, and they all run.
#[test]
fn waits_do_not_count_against_the_command_limit() {
    let mut calls: Vec<(String, &str, String)> = Vec::new();
    for i in 0..MAX_COMMANDS_PER_TURN {
        calls.push((format!("cmd_{i}"), "run_command", "{\"command\":\"pwd\"}".to_string()));
    }
    for i in 0..5 {
        calls.push((format!("wait_{i}"), "wait", "{\"seconds\":1}".to_string()));
    }
    let refs: Vec<(&str, &str, &str)> = calls.iter().map(|(id, name, args)| (id.as_str(), *name, args.as_str())).collect();
    let provider = serve(vec![Script::json(tool_reply(&refs)), Script::json(text_reply("done"))]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let gate = no_gate();
    gate.set_auto_approve(true);
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &tools, &gate, "run and wait");

    let reached = tools.reached();
    assert_eq!(reached.iter().filter(|n| *n == "run_command").count(), MAX_COMMANDS_PER_TURN);
    assert_eq!(reached.iter().filter(|n| *n == "wait").count(), 5, "a wait was refused");
    let msgs = stored(&conv.history());
    assert!(!format!("{msgs:?}").contains("per-turn command limit"), "a wait counted as a command");
    assert_paired(&msgs);
}

#[test]
fn streamed_tool_call_is_accumulated_and_answered() {
    let provider = serve(vec![Script::sse(SSE_CWD_CALL), Script::sse(&sse_text(&["done"]))]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, true), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "where am I");

    assert_eq!(deltas(&events), ["done"]);
    assert_eq!(decisions(&events), [("get_cwd".to_string(), "read", true)]);
    let msgs = stored(&conv.history());
    assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_s1");
    assert_eq!(msgs[1]["tool_calls"][0]["function"]["arguments"], "{}");
    assert_eq!(tool_content(&msgs, "call_s1"), "box:/home/u");
    assert_eq!(msgs[3]["content"], "done");
    assert_paired(&msgs);
}

#[test]
fn denied_run_command_ends_the_turn_and_closes_the_rest() {
    let provider = serve(vec![Script::json(tool_reply(&[
        ("call_a", "run_command", "{\"command\":\"pwd\"}"),
        ("call_b", "get_cwd", "{}"),
    ]))]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    // Nobody answers the approval, so it times out as a denial.
    let events = turn(&mut agent, &tools, &no_gate(), "run pwd");

    assert_eq!(provider.requests().len(), 1, "a denial must not lead to another model call");
    assert!(tools.reached().is_empty(), "nothing ran: {:?}", tools.reached());
    assert_eq!(decisions(&events), [("run_command".to_string(), "denied", false)]);
    assert_eq!(texts(&events).len(), 1, "one notice that the command was not approved");
    let msgs = stored(&conv.history());
    assert_eq!(msgs.len(), 4, "user, assistant, and one tool result per call");
    assert!(tool_content(&msgs, "call_a").starts_with("error: not_approved"), "{msgs:?}");
    assert_eq!(tool_content(&msgs, "call_b"), "denied by the user");
    assert_paired(&msgs);
}

#[test]
fn invalid_json_arguments_go_back_as_an_error_and_the_model_is_called_again() {
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_1", "get_cwd", "{not json")])),
        Script::json(text_reply("Sorry, let me try again.")),
    ]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &tools, &no_gate(), "where am I");

    assert!(tools.reached().is_empty());
    assert_eq!(decisions(&events), Vec::new(), "a call that never parsed is not a tool call in the UI");
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    let msgs = stored(&conv.history());
    assert_eq!(tool_content(&msgs, "call_1"), "error: arguments were not valid JSON");
    assert_paired(&msgs);
}

#[test]
fn unknown_tool_and_send_keys_are_errors_the_model_can_see() {
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_1", "frobnicate", "{}"), ("call_2", "send_keys", "{\"bytes\":[13]}")])),
        Script::json(text_reply("Understood.")),
    ]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &tools, &no_gate(), "do things");

    assert!(tools.reached().is_empty(), "neither call reached the tools: {:?}", tools.reached());
    assert_eq!(provider.requests().len(), 2);
    let msgs = stored(&conv.history());
    assert_eq!(tool_content(&msgs, "call_1"), "error: unknown tool `frobnicate`");
    assert_eq!(tool_content(&msgs, "call_2"), "error: send_keys is not available in this version");
    assert_paired(&msgs);
}

#[test]
fn provider_400_is_one_message_and_the_next_turn_works() {
    let provider = serve(vec![
        Script::error("400 Bad Request", json!({"error": {"message": "no-such-model is not a valid model ID", "code": 400}})),
        Script::json(text_reply("second answer")),
    ]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let first = turn(&mut agent, &FakeTools::default(), &no_gate(), "first");

    assert_eq!(texts(&first), ["The provider returned HTTP 400: no-such-model is not a valid model ID"]);
    assert!(matches!(first.last(), Some(AgentEvent::Done)));
    assert_eq!(stored(&conv.history()), vec![json!({"role": "user", "content": "first"})]);

    let second = turn(&mut agent, &FakeTools::default(), &no_gate(), "second");
    assert_eq!(texts(&second), ["second answer"]);
    let requests = provider.requests();
    let roles: Vec<String> = body_of(&requests[1])["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(roles, ["system", "user", "user"], "the failed turn left its user message and nothing pending");
}

#[test]
fn cancel_during_the_model_call_returns_promptly_and_keeps_the_history_valid() {
    let provider = serve(vec![Script::stall()]);
    let conv = Conversation::new().unwrap();
    let cancel = Arc::new(TurnCancel::new());
    let mut agent = agent_for(&settings(&provider.url, true), &conv, cancel.clone());
    let canceller = {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "hi");
    canceller.join().unwrap();

    assert!(started.elapsed() < Duration::from_secs(2), "cancel took {:?}", started.elapsed());
    assert!(texts(&events).is_empty() && deltas(&events).is_empty(), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Thinking { active: false })), "thinking must stop on cancel");
    assert_eq!(stored(&conv.history()), vec![json!({"role": "user", "content": "hi"})]);
}

#[test]
fn cancel_during_a_tool_call_closes_the_rest_without_another_model_call() {
    let provider = serve(vec![Script::json(tool_reply(&[("call_1", "get_cwd", "{}"), ("call_2", "get_cwd", "{}")]))]);
    let conv = Conversation::new().unwrap();
    let cancel = Arc::new(TurnCancel::new());
    let tools = FakeTools { cancel_on_cwd: Some(cancel.clone()), ..Default::default() };
    let mut agent = agent_for(&settings(&provider.url, false), &conv, cancel);
    let events = turn(&mut agent, &tools, &no_gate(), "where am I");

    assert_eq!(tools.reached(), ["get_cwd"], "the second call must not run after the cancel");
    assert_eq!(provider.requests().len(), 1);
    assert!(texts(&events).is_empty(), "a cancel says nothing in the chat: {events:?}");
    let msgs = stored(&conv.history());
    assert_eq!(tool_content(&msgs, "call_1"), "box:/home/u");
    assert_eq!(tool_content(&msgs, "call_2"), "cancelled");
    assert_paired(&msgs);
}

#[test]
fn the_command_past_the_per_turn_cap_gets_the_limit_error() {
    // One more call than the cap, all in one reply: the last one is refused, the others run.
    let over = MAX_COMMANDS_PER_TURN + 1;
    let calls: Vec<(String, &str, String)> =
        (1..=over).map(|n| (format!("call_{n}"), "run_command", format!("{{\"command\":\"echo {n}\"}}"))).collect();
    let call_refs: Vec<(&str, &str, &str)> = calls.iter().map(|(id, name, args)| (id.as_str(), *name, args.as_str())).collect();
    let provider = serve(vec![Script::json(tool_reply(&call_refs)), Script::json(text_reply("ok"))]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let gate = no_gate();
    gate.set_auto_approve(true);
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &tools, &gate, "run too many things");

    assert_eq!(tools.reached().iter().filter(|n| *n == "run_command").count(), MAX_COMMANDS_PER_TURN);
    let msgs = stored(&conv.history());
    assert!(tool_content(&msgs, &format!("call_{MAX_COMMANDS_PER_TURN}")).starts_with('{'), "the last allowed one ran");
    assert_eq!(
        tool_content(&msgs, &format!("call_{over}")),
        "error: per-turn command limit reached; ask the user to continue"
    );
    assert_paired(&msgs);
}

#[test]
fn the_model_call_cap_stops_the_turn_and_runs_nothing_after_it() {
    let scripts: Vec<Script> = (1..=MAX_MODEL_CALLS)
        .map(|n| Script::json(tool_reply(&[(&format!("call_{n}"), "get_cwd", "{}")])))
        .collect();
    let provider = serve(scripts);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &tools, &no_gate(), "keep looking");

    assert_eq!(provider.requests().len(), MAX_MODEL_CALLS);
    assert_eq!(tools.reached().len(), MAX_MODEL_CALLS - 1, "the last reply's calls are not run");
    assert_eq!(texts(&events), [format!("I stopped after {MAX_MODEL_CALLS} model calls in one turn.")]);
    let msgs = stored(&conv.history());
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "tool");
    assert!(last["content"].as_str().unwrap().contains("model call limit"), "{last}");
    assert_paired(&msgs);
}

#[test]
fn the_request_carries_the_system_prompt_tools_auth_session_header_and_history() {
    let provider = serve(vec![Script::json(text_reply("first answer")), Script::json(text_reply("second answer"))]);
    let conv = Conversation::new().unwrap();
    let mut s = settings(&provider.url, false);
    s.headers = vec![("x-opencode-session".to_string(), "{session_id}".to_string())];
    let mut agent = agent_for(&s, &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &FakeTools::default(), &no_gate(), "first question");
    turn(&mut agent, &FakeTools::default(), &no_gate(), "second question");

    let requests = provider.requests();
    assert_eq!(header_of(&requests[1], "authorization"), Some(format!("Bearer {KEY}")));
    assert_eq!(header_of(&requests[1], "x-opencode-session"), Some(conv.id()), "the placeholder is the conversation id");
    assert!(conv.id().starts_with("ses_") && conv.id().len() == 36);
    assert!(header_of(&requests[1], "user-agent").is_some_and(|ua| ua.starts_with("agent-term/")));

    let first = body_of(&requests[0]);
    assert_eq!(first["model"], MODEL);
    assert_eq!(first["stream"], false);
    let names: Vec<&str> = first["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"get_cwd") && names.contains(&"run_command"), "{names:?}");
    assert!(!names.contains(&"send_keys"), "send_keys is not advertised: {names:?}");
    assert_eq!(names.len(), prompt::tool_definitions().len());

    let second = body_of(&requests[1]);
    let messages = second["messages"].as_array().unwrap();
    assert_eq!(messages[0], json!({"role": "system", "content": prompt::system_prompt()}));
    assert_eq!(messages[1], json!({"role": "user", "content": "first question"}));
    assert_eq!(messages[2], json!({"role": "assistant", "content": "first answer"}));
    assert_eq!(messages[3], json!({"role": "user", "content": "second question"}));
}

#[test]
fn the_key_never_reaches_events_or_history() {
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_1", "run_command", "{\"command\":\"echo token\"}")])),
        Script::json(text_reply("done")),
        Script::error("401 Unauthorized", json!({"error": {"message": format!("invalid key {KEY}"), "code": 401}})),
    ]);
    let conv = Conversation::new().unwrap();
    // The command's output contains the key, as if a program printed it.
    let tools = FakeTools { command_output: Some(format!("token is {KEY} end\n")), ..Default::default() };
    let gate = no_gate();
    gate.set_auto_approve(true);
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));

    let mut all_events = Vec::new();
    all_events.extend(turn(&mut agent, &tools, &gate, "print the token"));
    all_events.extend(turn(&mut agent, &tools, &gate, "thanks"));
    all_events.extend(turn(&mut agent, &tools, &gate, "again"));
    // The ToolResult event carries the tool's full result by design (the UI shows what the terminal shows), so the
    // printed copy of the key is allowed there. Every other event must be free of it.
    let other_events: Vec<&AgentEvent> = all_events.iter().filter(|e| !matches!(e, AgentEvent::ToolResult { .. })).collect();
    let serialized_events = serde_json::to_string(&other_events).unwrap();
    assert!(!serialized_events.contains(KEY), "the key reached an event other than the raw tool result");
    assert!(tool_result_has_raw_output(&all_events), "the UI's ToolResult keeps the full result");
    assert!(texts(&all_events).iter().any(|t| t.starts_with("The provider returned HTTP 401")));

    let stored_text = serde_json::to_string(&stored(&conv.history())).unwrap();
    assert!(!stored_text.contains(KEY), "the key reached the history");
    assert!(stored_text.contains("[REDACTED]"), "the printed key was replaced in the tool result");

    let requests = provider.requests();
    let body_after_tool = requests[1].split_once("\r\n\r\n").unwrap().1;
    assert!(!body_after_tool.contains(KEY), "the key reached the model");
}

#[test]
fn new_conversation_clears_the_history_and_rotates_the_id() {
    let provider = serve(vec![Script::json(text_reply("one"))]);
    let conv = Conversation::new().unwrap();
    let before = conv.id();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &FakeTools::default(), &no_gate(), "hello");
    assert_eq!(stored(&conv.history()).len(), 2);

    conv.reset().unwrap();
    assert!(stored(&conv.history()).is_empty());
    assert_ne!(conv.id(), before);
}

/// The arguments of a `todo_write` call, as the model sends them (a JSON string).
fn todo_args(items: &[(&str, &str)]) -> String {
    let items: Vec<Value> = items.iter().map(|(content, status)| json!({"content": content, "status": status})).collect();
    json!({"items": items}).to_string()
}

/// The system message of a request, as the provider received it.
fn system_of(raw: &str) -> String {
    body_of(raw)["messages"][0]["content"].as_str().expect("system content").to_string()
}

const LIST_BLOCK: &str = "Your todo list (update it with todo_write):\nCurrent: fix the bug\nDone: read the code\nPending: run the tests";

#[test]
fn todo_write_replaces_the_list_is_shown_on_the_next_call_and_is_reported_as_a_tool() {
    let args = todo_args(&[("read the code", "completed"), ("fix the bug", "in_progress"), ("run the tests", "pending")]);
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_t1", "todo_write", &args)])),
        Script::json(text_reply("working on it")),
    ]);
    let conv = Conversation::new().unwrap();
    let tools = FakeTools::default();
    let gate = no_gate();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &tools, &gate, "plan the work");

    // Handled inside the loop: no backend call, no approval card, and no command counted.
    assert!(tools.reached().is_empty(), "todo_write reached the backend: {:?}", tools.reached());
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. } | AgentEvent::AgentControl { .. })));
    assert_eq!(decisions(&events), [("todo_write".to_string(), "read", true)]);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCall { name, .. } if name == "todo_write")));
    let update: Vec<&Vec<todo::Todo>> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TodoUpdate { items } => Some(items),
            _ => None,
        })
        .collect();
    assert_eq!(update.len(), 1, "one update event for one successful call");
    assert_eq!(update[0].len(), 3);
    assert_eq!(update[0][1].status, todo::TodoStatus::InProgress);
    assert_eq!(conv.todos().lock().unwrap().clone(), update[0].clone(), "the conversation holds the list");

    // The model is told the outcome under its call id, and the next request carries the list.
    let requests = provider.requests();
    assert!(body_of(&requests[0])["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "todo_write"));
    assert_eq!(system_of(&requests[1]), format!("{}\n\n{LIST_BLOCK}", prompt::system_prompt()));
    let msgs = stored(&conv.history());
    assert_paired(&msgs);
    assert_eq!(tool_content(&msgs, "call_t1"), "todo list updated: 1 done, 1 in progress, 1 pending");
    assert!(!serde_json::to_string(&msgs).unwrap().contains("Your todo list"), "the block is never stored");
}

#[test]
fn the_list_survives_across_turns_and_an_empty_list_clears_the_block() {
    let first = todo_args(&[("read the code", "completed"), ("fix the bug", "in_progress")]);
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_a", "todo_write", &first)])),
        Script::json(text_reply("step one done")),
        // Second turn: no tool call, the list is still in the request.
        Script::json(text_reply("still going")),
        // Third turn: the model clears the list.
        Script::json(tool_reply(&[("call_b", "todo_write", "{\"items\":[]}")])),
        Script::json(text_reply("cleared")),
    ]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    turn(&mut agent, &FakeTools::default(), &no_gate(), "start");
    turn(&mut agent, &FakeTools::default(), &no_gate(), "continue");
    let requests = provider.requests();
    assert_eq!(system_of(&requests[2]), format!("{}\n\n{}", prompt::system_prompt(), "Your todo list (update it with todo_write):\nCurrent: fix the bug\nDone: read the code"));

    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "finish up");
    let requests = provider.requests();
    assert_eq!(requests.len(), 5);
    assert!(conv.todos().lock().unwrap().clone().is_empty(), "the empty list cleared the conversation's list");
    let cleared = events.iter().any(|e| matches!(e, AgentEvent::TodoUpdate { items } if items.is_empty()));
    assert!(cleared, "clearing is reported as an update with no items");
    assert_eq!(system_of(&requests[4]), prompt::system_prompt(), "no block once the list is empty");
}

#[test]
fn an_invalid_todo_write_changes_nothing_and_the_model_reads_why() {
    let bad = todo_args(&[("a", "in_progress"), ("b", "in_progress")]);
    let provider = serve(vec![
        Script::json(tool_reply(&[("call_bad", "todo_write", &bad)])),
        Script::json(text_reply("I will fix that")),
    ]);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&settings(&provider.url, false), &conv, Arc::new(TurnCancel::new()));
    let events = turn(&mut agent, &FakeTools::default(), &no_gate(), "plan");

    assert!(!events.iter().any(|e| matches!(e, AgentEvent::TodoUpdate { .. })), "a refused list is not announced");
    assert_eq!(decisions(&events), [("todo_write".to_string(), "read", false)]);
    assert!(conv.todos().lock().unwrap().clone().is_empty());
    let requests = provider.requests();
    assert_eq!(system_of(&requests[1]), prompt::system_prompt());
    let msgs = stored(&conv.history());
    assert_paired(&msgs);
    let text = tool_content(&msgs, "call_bad");
    assert!(text.starts_with("error: ") && text.contains("exactly one in_progress"), "{text}");
}
