//! The chat agent. Most modules do no I/O: how tool results are shaped for the model, how secrets are redacted,
//! the conversation history with its tool-call pairing, and the prompt. `agent_loop` runs a turn over them and
//! over the provider adapter (core/llm).

pub mod agent_loop;
pub mod history;
pub mod project;
pub mod prompt;
pub mod redact;
