//! PTY reading, coalescing, and flow control.
//!
//! Two threads read the master side. `pty-read` does blocking reads and hands chunks over a bounded
//! channel. `pty-pump` coalesces them (up to 32 KiB or 4 ms, because macOS PTY reads can be a few
//! bytes each), waits on the gate, passes the batch to the output sink, and then pushes a copy to the
//! observer queue without blocking.
//!
//! The gate is the back-pressure path. The sink has to call `Session::ack` once it has rendered bytes.
//! Above 2 MiB unacked, the pump stops (and so does the child, through the kernel PTY buffer). It
//! resumes below 512 KiB. Acks are sent in byte counts, never per chunk.

use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::core::observer::{ObsMsg, ObsQueue};

pub const HIGH_WATERMARK: usize = 2 << 20;
pub const LOW_WATERMARK: usize = 512 << 10;
pub const COALESCE_BYTES: usize = 32 << 10;
pub const COALESCE_TIME: Duration = Duration::from_millis(4);
const READ_BUF: usize = 64 << 10;

/// Receives output in PTY order. The implementation must acknowledge rendered bytes via `Session::ack`.
pub trait OutputSink: Send {
    fn output(&mut self, bytes: &[u8]);
}

pub struct Gate {
    state: Mutex<GateState>,
    cv: Condvar,
}

#[derive(Default)]
struct GateState {
    unacked: usize,
    paused: bool,
    closed: bool,
}

impl Gate {
    pub fn new() -> Self {
        Gate { state: Mutex::new(GateState::default()), cv: Condvar::new() }
    }

    /// Blocks while paused. Returns false once the gate is closed.
    fn wait_for_room(&self) -> bool {
        let mut s = self.lock();
        while s.paused && !s.closed {
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        !s.closed
    }

    fn sent(&self, n: usize) {
        let mut s = self.lock();
        s.unacked += n;
        if s.unacked > HIGH_WATERMARK {
            s.paused = true;
        }
    }

    pub fn ack(&self, n: usize) {
        let mut s = self.lock();
        s.unacked = s.unacked.saturating_sub(n);
        if s.paused && s.unacked < LOW_WATERMARK {
            s.paused = false;
            self.cv.notify_all();
        }
    }

    /// Wakes a blocked pump so it can exit (session teardown).
    pub fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

enum ReadMsg {
    Data(Vec<u8>),
    Eof,
}

/// Starts the reader and pump threads. They exit on EOF, or when the gate is closed.
pub fn start_threads(
    mut reader: Box<dyn Read + Send>,
    gate: Arc<Gate>,
    mut sink: Box<dyn OutputSink>,
    queue: Arc<ObsQueue>,
) -> io::Result<Vec<JoinHandle<()>>> {
    let (tx, rx) = mpsc::sync_channel::<ReadMsg>(8);
    let read_handle = thread::Builder::new().name("pty-read".into()).spawn(move || {
        let mut buf = vec![0u8; READ_BUF];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(ReadMsg::Eof);
                    break;
                }
                Ok(n) => {
                    if tx.send(ReadMsg::Data(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // EIO after the child exits is the normal end of the stream on some platforms.
                Err(_) => {
                    let _ = tx.send(ReadMsg::Eof);
                    break;
                }
            }
        }
    })?;
    let pump_handle = thread::Builder::new().name("pty-pump".into()).spawn(move || pump(rx, gate, &mut *sink, queue))?;
    Ok(vec![read_handle, pump_handle])
}

fn pump(rx: Receiver<ReadMsg>, gate: Arc<Gate>, sink: &mut dyn OutputSink, queue: Arc<ObsQueue>) {
    let mut eof = false;
    while !eof {
        let mut batch = match rx.recv() {
            Ok(ReadMsg::Data(d)) => d,
            Ok(ReadMsg::Eof) | Err(_) => break,
        };
        let deadline = Instant::now() + COALESCE_TIME;
        while batch.len() < COALESCE_BYTES {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(ReadMsg::Data(d)) => batch.extend_from_slice(&d),
                Ok(ReadMsg::Eof) => {
                    eof = true;
                    break;
                }
                Err(_) => break,
            }
        }
        if !gate.wait_for_room() {
            // Closed (session teardown): keep draining until EOF, so a dying shell can flush its tty and exit,
            // but send nothing more.
            continue;
        }
        sink.output(&batch);
        gate.sent(batch.len());
        queue.push_bytes(batch);
    }
    queue.push_control(ObsMsg::Eof);
}
