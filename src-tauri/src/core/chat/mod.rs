//! Pieces of the chat agent that do no I/O: how tool results are shaped for the model, how secrets are redacted,
//! the conversation history with its tool-call pairing, and the prompt. The loop that uses them is elsewhere.

pub mod history;
pub mod project;
pub mod prompt;
pub mod redact;
