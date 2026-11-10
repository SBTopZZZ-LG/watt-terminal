//! Persistence end to end, without Tauri: a turn of the LLM agent against a local fake provider, saved with
//! `save_turn`, then resumed into a live conversation with `resume` and `Conversation::restore`. The provider is a std
//! TcpListener on 127.0.0.1 that answers each request with the next scripted reply and keeps the raw request, as in
//! tests/agent_loop.rs. No shell is involved and no network is used.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent_term_lib::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate};
use agent_term_lib::core::chat::agent_loop::{Conversation, LlmAgent, LlmSettings};
use agent_term_lib::core::chat::redact::Redactor;
use agent_term_lib::core::llm::{TurnCancel, http_client};
use agent_term_lib::core::persist::conversations::{ConversationMeta, SaveRequest};
use agent_term_lib::core::persist::turn::{FinishedTurn, TurnEnv, resume, save_turn};
use agent_term_lib::core::persist::workspace::Workspace;
use agent_term_lib::core::persist::{Persist, PersistError};
use agent_term_lib::core::record::{Cwd, RecordSummary};
use agent_term_lib::core::tools::{AgentTools, CommandView, RunOutcome, ScreenView};
use serde_json::{Value, json};

const KEY: &str = "sk-or-test-SECRETVALUE-0123456789";
const MODEL: &str = "deepseek/deepseek-v4.1-flash";

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("agent-term-persist-it-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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

/// Serves one non-streamed JSON reply per connection, in order.
fn serve(replies: Vec<Value>) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let url = format!("http://{}/v1/chat/completions", listener.local_addr().expect("local addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let _ = sock.set_nodelay(true);
            log.lock().unwrap().push(read_request(&mut sock));
            let body = reply.to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
            let _ = sock.flush();
        }
    });
    Provider { url, requests }
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
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
}

/// A non-streamed plain answer.
fn text_reply(text: &str) -> Value {
    json!({"choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": text}}]})
}

/// No tool is called in these tests, so every tool method is unreachable.
struct NoTools;

impl AgentTools for NoTools {
    fn list_commands(&self, _n: usize) -> Vec<RecordSummary> {
        unreachable!("no tool is called in these tests")
    }
    fn get_command(&self, _id: u64, _offset: usize, _limit: usize) -> Result<CommandView, String> {
        unreachable!("no tool is called in these tests")
    }
    fn get_screen(&self) -> ScreenView {
        unreachable!("no tool is called in these tests")
    }
    fn get_cwd(&self) -> Option<Cwd> {
        unreachable!("no tool is called in these tests")
    }
    fn run_command(&self, _command: &str, _timeout: Duration) -> Result<RunOutcome, String> {
        unreachable!("no tool is called in these tests")
    }
    fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
        unreachable!("no tool is called in these tests")
    }
}

fn settings(url: &str) -> LlmSettings {
    LlmSettings {
        chat_url: url.to_string(),
        api_key: KEY.to_string(),
        model: MODEL.to_string(),
        headers: vec![("x-session".to_string(), "{session_id}".to_string())],
        stream: false,
        max_tokens: None,
    }
}

fn agent_for(s: &LlmSettings, conv: &Conversation) -> LlmAgent {
    LlmAgent::new(&http_client().expect("client"), s, &conv.id(), conv.history(), Arc::new(TurnCancel::new()))
}

/// Runs one turn of the agent, as the app does on its agent thread.
fn run(agent: &mut LlmAgent, text: &str) {
    let tools = NoTools;
    let gate = ApprovalGate::new();
    let emit = |_: AgentEvent| {};
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    agent::run_turn(agent, text, &ctx);
}

/// The end-of-turn save, as the app does it: the history snapshot, the agent's redactor, and the shell's directory.
fn finish(
    persist: &Persist,
    conv: &Conversation,
    agent: &LlmAgent,
    cwd: Option<(&str, &str)>,
    now: u64,
) -> Option<ConversationMeta> {
    let id = conv.id();
    let messages = conv.history().lock().unwrap().snapshot();
    let turn = FinishedTurn { conversation_id: &id, messages, model: Some(MODEL.to_string()), cwd };
    let env = TurnEnv { local_host: "this-mac", home: None };
    save_turn(persist, turn, agent.redactor(), env, now).expect("save succeeds")
}

/// The request's messages, system message included, as the model received them.
fn sent_messages(raw: &str) -> Vec<Value> {
    body_of(raw)["messages"].as_array().expect("messages array").clone()
}

/// Every file under `dir`, concatenated. Empty when the directory does not exist.
fn all_text(dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(dir) else { return String::new() };
    let mut out = String::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_text(&path));
        } else if let Ok(text) = fs::read_to_string(&path) {
            out.push_str(&text);
        }
    }
    out
}

fn persist_in(tmp: &TempDir) -> Persist {
    Persist::new(&tmp.path().join("config"), &tmp.path().join("data"))
}

#[test]
fn a_resumed_conversation_gives_the_model_its_earlier_messages() {
    let provider = serve(vec![text_reply("first answer"), text_reply("second answer")]);
    let tmp = TempDir::new();
    let persist = persist_in(&tmp);
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let repo_text = repo.to_string_lossy().into_owned();
    let s = settings(&provider.url);

    // Turn 1 in the first conversation, then its end-of-turn save.
    let first = Conversation::new().unwrap();
    let mut agent = agent_for(&s, &first);
    run(&mut agent, "first question");
    let saved = finish(&persist, &first, &agent, Some(("", &repo_text)), 100).expect("the turn is saved");
    assert_eq!(saved.id, first.id());
    assert_eq!(saved.workspace.label, "repo");
    assert_eq!(saved.message_count, 2);

    // Resume it into a fresh live conversation, as load_conversation does, then run a second turn.
    let loaded = resume(&persist, &saved.id).expect("resume");
    let live = Conversation::new().unwrap();
    live.restore(loaded.id, loaded.history);
    assert_eq!(live.id(), first.id(), "the provider session id is kept");
    let mut resumed_agent = agent_for(&s, &live);
    run(&mut resumed_agent, "second question");

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(header_of(&requests[1], "x-session"), Some(first.id()), "same session header after resume");
    let messages = sent_messages(&requests[1]);
    assert_eq!(messages.len(), 4, "system, the earlier exchange, and the new question: {messages:?}");
    assert_eq!(messages[1], json!({"role": "user", "content": "first question"}));
    assert_eq!(messages[2], json!({"role": "assistant", "content": "first answer"}));
    assert_eq!(messages[3], json!({"role": "user", "content": "second question"}));

    // The second turn saves over the first: same id, same creation time and title, the new message count.
    let resaved = finish(&persist, &live, &resumed_agent, Some(("", &repo_text)), 200).expect("saved again");
    assert_eq!(resaved.id, first.id());
    assert_eq!(resaved.created_at, 100);
    assert_eq!(resaved.updated_at, 200);
    assert_eq!(resaved.title, "first question");
    assert_eq!(resaved.message_count, 4);
}

#[test]
fn the_key_never_reaches_a_saved_file() {
    // The provider's reply repeats the key, and so does the user's message.
    let provider = serve(vec![text_reply(&format!("the key is {KEY}, keep it"))]);
    let tmp = TempDir::new();
    let persist = persist_in(&tmp);
    let s = settings(&provider.url);
    let conv = Conversation::new().unwrap();
    let mut agent = agent_for(&s, &conv);
    run(&mut agent, &format!("my key is {KEY}"));
    finish(&persist, &conv, &agent, None, 1).expect("saved");

    let on_disk = all_text(tmp.path());
    assert!(!on_disk.is_empty(), "something was written");
    assert!(!on_disk.contains(KEY), "the key was written to disk");
    assert!(on_disk.contains("[REDACTED]"));

    let resumed = resume(&persist, &conv.id()).expect("resume");
    let snapshot = resumed.history.snapshot();
    assert_eq!(snapshot[0]["content"], "my key is [REDACTED]", "the user message is redacted on save");
    // The adapter already scrubs the key from the raw reply body (core/llm), so the marker there is its own.
    let reply = snapshot[1]["content"].as_str().expect("assistant text");
    assert!(!reply.contains(KEY) && reply.contains("keep it"), "{reply}");
}

#[test]
fn a_corrupt_conversation_file_is_an_error_and_the_live_conversation_is_untouched() {
    let tmp = TempDir::new();
    let persist = persist_in(&tmp);
    let live = Conversation::new().unwrap();
    live.history().lock().unwrap().push_user("still here");
    let live_id = live.id();
    let damaged_id = format!("ses_{:032x}", 9);
    let damaged = tmp.path().join("data").join("conversations").join(format!("{damaged_id}.json"));
    fs::create_dir_all(damaged.parent().unwrap()).unwrap();
    fs::write(&damaged, b"{\"version\": 1, \"id\": ").unwrap();

    // The command loads (and only then restores), so a failed load leaves the live conversation alone.
    match resume(&persist, &damaged_id) {
        Err(PersistError::Corrupt(_)) => {}
        Err(other) => panic!("expected Corrupt, got {other:?}"),
        Ok(_) => panic!("a damaged file must not load"),
    }
    assert_eq!(live.id(), live_id);
    let snapshot = live.history().lock().unwrap().snapshot();
    assert_eq!(snapshot, vec![json!({"role": "user", "content": "still here"})]);
}

#[test]
fn a_saved_unanswered_tool_call_is_closed_before_the_model_sees_the_resumed_history() {
    let provider = serve(vec![text_reply("continuing")]);
    let tmp = TempDir::new();
    let persist = persist_in(&tmp);
    // A conversation file as an older build might leave it: a call with no result.
    let id = format!("ses_{:032x}", 11);
    let stored_messages = vec![
        json!({"role": "user", "content": "check the disk"}),
        json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "call_x", "type": "function", "function": {"name": "run_command", "arguments": "{}"}}
        ]}),
    ];
    persist
        .conversations
        .save(
            &SaveRequest {
                id: id.clone(),
                workspace: Workspace::unknown(),
                model: Some(MODEL.to_string()),
                messages: stored_messages,
            },
            &Redactor::new(vec![KEY.to_string()]),
            5,
        )
        .expect("save the old file");

    let loaded = resume(&persist, &id).expect("resume");
    let live = Conversation::new().unwrap();
    live.restore(loaded.id, loaded.history);
    let s = settings(&provider.url);
    let mut agent = agent_for(&s, &live);
    run(&mut agent, "and now?");

    let messages = sent_messages(&provider.requests()[0]);
    let closing = messages.iter().find(|m| m["tool_call_id"] == "call_x").expect("the call is closed");
    assert_eq!(closing["role"], "tool");
    assert_eq!(closing["content"], "skipped: not recorded");
    assert_eq!(messages.last().unwrap(), &json!({"role": "user", "content": "and now?"}));
}
