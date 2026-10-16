// Kudu Desktop agent - ToolOutcome.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

/// A tool's result: JSON for the model, plus an optional screenshot to show it.
pub(crate) struct ToolOutcome {
    pub(crate) response: Value,
    pub(crate) image_data_url: Option<String>,
    pub(crate) log: String,
}

impl ToolOutcome {
    pub(crate) fn text(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        ToolOutcome { response: json!({ "result": msg.clone() }), image_data_url: None, log: msg }
    }
    pub(crate) fn err(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        ToolOutcome { response: json!({ "error": msg.clone() }), image_data_url: None, log: format!("error: {}", msg) }
    }
}
