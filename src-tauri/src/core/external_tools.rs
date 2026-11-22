//! The seam for tools the app does not own (MCP servers will plug in here). Nothing implements `ExternalToolHost` in the
//! app yet: the agent only consults a host that a caller attaches to its `AgentCtx` (see `AgentCtx::with_external_tools`).
//!
//! A host's tools are offered to the model next to the built-in ones, and a call to one goes through the approval gate
//! (core/agent). A name that is not a built-in and that no host owns is refused. A built-in name always wins over a host
//! tool with the same name.

use serde_json::Value;

use crate::core::llm::TurnCancel;

/// One tool a host offers. Owned strings: the names are not known at compile time.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalToolSchema {
    pub name: String,
    pub description: String,
    /// The JSON Schema of the arguments, as the model sees it.
    pub parameters: Value,
}

/// How a host tool is approved. `Never` hides the tool from the model and refuses a call to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalApproval {
    /// The user is asked before each call (unless approve-all is on).
    Ask,
    /// The call runs without a card.
    Auto,
    /// The tool is disabled.
    Never,
}

/// Names the server and the tool, for the approval card and the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDisplay {
    pub server: String,
    pub tool: String,
}

/// Tools the app does not own. Calls block, so the agent thread waits for `call` to return. The host owns its own
/// timeouts; the gate adds none.
pub trait ExternalToolHost: Send + Sync {
    /// The tools offered now. Read at each model request, so the list may change between requests.
    fn schemas(&self) -> Vec<ExternalToolSchema>;
    /// Whether this host handles `name`.
    fn owns(&self, name: &str) -> bool;
    /// The approval for `name`. Only asked for names this host owns.
    fn approval(&self, name: &str) -> ExternalApproval;
    /// The server and tool names of one call, for the card.
    fn display(&self, name: &str, args: &Value) -> ExternalDisplay;
    /// Runs one call. `Ok` is the text the model reads (redacted and capped by the agent); `Err` is the error text.
    fn call(&self, name: &str, args: Value, cancel: &TurnCancel) -> Result<String, String>;
}

/// A fake host for the tests of the agent and the agent loop. Nothing here touches the app or a server.
#[cfg(test)]
pub mod testing {
    use std::sync::Mutex;

    use super::*;

    /// Serves fixed tools and records every call that reaches `call`.
    pub struct FakeHost {
        /// The tools `schemas` returns. A test may replace them between two requests.
        pub tools: Mutex<Vec<(String, ExternalApproval)>>,
        /// What `call` returns on success.
        pub output: String,
        /// The `name` of every call that reached `call`, in order.
        pub reached: Mutex<Vec<String>>,
    }

    impl FakeHost {
        pub fn new(tools: &[(&str, ExternalApproval)], output: &str) -> Self {
            FakeHost {
                tools: Mutex::new(tools.iter().map(|(n, a)| (n.to_string(), *a)).collect()),
                output: output.to_string(),
                reached: Mutex::new(Vec::new()),
            }
        }

        pub fn reached(&self) -> Vec<String> {
            self.reached.lock().unwrap().clone()
        }
    }

    impl ExternalToolHost for FakeHost {
        fn schemas(&self) -> Vec<ExternalToolSchema> {
            self.tools
                .lock()
                .unwrap()
                .iter()
                .map(|(name, _)| ExternalToolSchema {
                    name: name.clone(),
                    description: format!("fake {name}"),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                })
                .collect()
        }

        fn owns(&self, name: &str) -> bool {
            self.tools.lock().unwrap().iter().any(|(n, _)| n == name)
        }

        fn approval(&self, name: &str) -> ExternalApproval {
            self.tools.lock().unwrap().iter().find(|(n, _)| n == name).map_or(ExternalApproval::Never, |(_, a)| *a)
        }

        fn display(&self, name: &str, _args: &Value) -> ExternalDisplay {
            ExternalDisplay { server: "fake-server".to_string(), tool: name.to_string() }
        }

        fn call(&self, name: &str, _args: Value, _cancel: &TurnCancel) -> Result<String, String> {
            self.reached.lock().unwrap().push(name.to_string());
            Ok(self.output.clone())
        }
    }
}
