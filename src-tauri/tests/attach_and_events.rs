//! Real-PTY tests for attach idempotence (the registry behind start_session) and the session event stream.

mod common;

use std::fs;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use agent_term_lib::core::observer::{EventSink, InputState, IntegrationStatus, SessionEvent};
use agent_term_lib::core::registry::Registry;
use agent_term_lib::core::session::{Session, SpawnConfig};

use common::*;

const ID: &str = "attach-test";

/// Spawns with a sink whose receiver is dropped, so output is discarded (these tests do not render it).
fn spawn_session(home: &std::path::Path) -> io::Result<Session> {
    let (tx, _rx) = mpsc::channel::<Vec<u8>>();
    let cfg = SpawnConfig { rows: 24, cols: 80, real_zdotdir: Some(home.to_path_buf()), histfile: None };
    Session::spawn(ID, cfg, Box::new(ChanSink(tx)))
}

#[test]
fn attaching_the_same_id_twice_spawns_one_pty() {
    let _g = serial();
    let home = make_real_home();
    let registry: Registry<Session> = Registry::new();
    let spawns = AtomicUsize::new(0);
    let spawn = || {
        spawns.fetch_add(1, Ordering::SeqCst);
        spawn_session(&home)
    };

    let (first, created) = registry.get_or_insert_with(ID, spawn).expect("first attach");
    let (second, created_again) = registry.get_or_insert_with(ID, spawn).expect("second attach");
    assert!(created);
    assert!(!created_again, "a second attach must not report a new session");
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "a second attach spawned another PTY");
    assert!(wait_state(&first, InputState::Prompt, Duration::from_secs(20)), "state={:?}", first.state());

    drop(second);
    registry.remove(ID);
    drop(first);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn nonce_is_stable_across_attach_and_accepted_by_the_shell() {
    let _g = serial();
    let home = make_real_home();
    let registry: Registry<Session> = Registry::new();
    let (first, created) = registry.get_or_insert_with(ID, || spawn_session(&home)).expect("first attach");
    assert!(created);
    let nonce = first.nonce().to_string();
    assert_eq!(nonce.len(), 32, "nonce is 128 bits as hex");
    assert!(nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "nonce is lowercase hex");
    // A second attach gets the same session, so the same nonce.
    let (second, created_again) = registry.get_or_insert_with(ID, || spawn_session(&home)).expect("second attach");
    assert!(!created_again);
    assert_eq!(second.nonce(), nonce);
    // The shell only trusts nonce-tagged Hello and prompt events, so reaching Prompt means it was given this nonce.
    assert!(wait_state(&first, InputState::Prompt, Duration::from_secs(20)), "state={:?}", first.state());
    assert_eq!(first.integration_status(), IntegrationStatus::Live);

    drop(second);
    registry.remove(ID);
    drop(first);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn concurrent_attaches_spawn_one_pty() {
    let _g = serial();
    let home = make_real_home();
    let registry: Arc<Registry<Session>> = Arc::new(Registry::new());
    let spawns = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (registry, spawns, barrier, home) = (registry.clone(), spawns.clone(), barrier.clone(), home.clone());
            thread::spawn(move || {
                barrier.wait();
                registry.get_or_insert_with(ID, || {
                    spawns.fetch_add(1, Ordering::SeqCst);
                    spawn_session(&home)
                })
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().expect("thread").expect("attach")).collect();
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "concurrent attaches spawned more than one PTY");
    assert_eq!(results.iter().filter(|(_, created)| *created).count(), 1);
    let first = &results[0].0;
    assert!(results.iter().all(|(s, _)| Arc::ptr_eq(s, first)));
    registry.remove(ID);
    let _ = fs::remove_dir_all(&home);
}

struct Collect(Mutex<Vec<SessionEvent>>);

impl EventSink for Collect {
    fn event(&self, ev: SessionEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

#[test]
fn listeners_get_state_integration_and_record_events() {
    let _g = serial();
    let s = start();
    // Attached after spawn: the listener must still learn the current state.
    let sink = Arc::new(Collect(Mutex::new(Vec::new())));
    s.set_event_sink(sink.clone());
    run(&s, "echo ev");

    let events = wait_until(Duration::from_secs(10), || {
        let evs = sink.0.lock().unwrap().clone();
        let done = evs.iter().any(|e| matches!(e, SessionEvent::RecordFinished { record } if record.command == "echo ev"));
        let prompt = evs.iter().any(|e| matches!(e, SessionEvent::State { state: InputState::Prompt }));
        (done && prompt).then_some(evs)
    })
    .expect("finished record and prompt state were delivered");

    assert!(events.iter().any(|e| matches!(e, SessionEvent::Integration { status: IntegrationStatus::Live })), "{events:?}");
    let started = events.iter().position(|e| matches!(e, SessionEvent::RecordStarted { command, .. } if command == "echo ev"));
    let finished = events.iter().position(|e| matches!(e, SessionEvent::RecordFinished { .. }));
    assert!(started.is_some() && started < finished, "record started must precede finished: {events:?}");
    let exit = events.iter().find_map(|e| match e {
        SessionEvent::RecordFinished { record } => Some(record.exit_code),
        _ => None,
    });
    assert_eq!(exit, Some(Some(0)));
}

#[test]
fn user_typed_command_is_reported_with_user_source() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let sink = Arc::new(Collect(Mutex::new(Vec::new())));
    s.set_event_sink(sink.clone());
    s.write_user(b"echo typed-ev\r").expect("typed");
    let started = wait_until(Duration::from_secs(10), || {
        sink.0.lock().unwrap().iter().find_map(|e| match e {
            SessionEvent::RecordStarted { command, source, .. } if command == "echo typed-ev" => Some(source.clone()),
            _ => None,
        })
    });
    assert_eq!(started, Some(agent_term_lib::core::record::Source::User));
}
