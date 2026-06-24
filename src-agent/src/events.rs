//! UI-agnostic event sink. The core agent loop reports progress through this
//! trait; the consumer (Tauri backend or the CLI harness) decides how to
//! surface it. The event names/shapes mirror the NDJSON protocol the old
//! Python sidecar used, so the Vue frontend can keep the same handling.

use serde_json::Value;

/// Sink for agent progress events. Must be cheap to clone-share (`Arc<dyn>`).
pub trait Events: Send + Sync {
    /// Generic progress / info line: `{"step": text}`.
    fn step(&self, text: &str);
    /// A step began with its goal: `{"step_start": goal, "step_id": id}`.
    fn step_start(&self, goal: &str, id: u32);
    /// A step finished: `{"step_done": {"id", "success", "error"}}`.
    fn step_done(&self, id: u32, success: bool, error: Option<&str>);
    /// Agent needs a human: `{"ask_human": question}`.
    fn ask_human(&self, question: &str);
    /// Finished successfully: `{"done": true, "result": result}`.
    fn done(&self, result: &str);
    /// Fatal error: `{"error": text}`.
    fn error(&self, text: &str);
    /// Non-fatal error log: `{"log_error": text}`.
    fn log_error(&self, text: &str);
}

/// Build the JSON payloads in one place so every consumer is byte-identical to
/// the legacy protocol. Consumers that emit `Value` (e.g. Tauri events) reuse
/// these; the CLI harness serializes them to NDJSON lines.
pub mod payload {
    use serde_json::{json, Value};

    pub fn step(text: &str) -> Value {
        json!({ "step": text })
    }
    pub fn step_start(goal: &str, id: u32) -> Value {
        json!({ "step_start": goal, "step_id": id })
    }
    pub fn step_done(id: u32, success: bool, error: Option<&str>) -> Value {
        json!({ "step_done": { "id": id, "success": success, "error": error } })
    }
    pub fn ask_human(question: &str) -> Value {
        json!({ "ask_human": question })
    }
    pub fn done(result: &str) -> Value {
        json!({ "done": true, "result": result })
    }
    pub fn error(text: &str) -> Value {
        json!({ "error": text })
    }
    pub fn log_error(text: &str) -> Value {
        json!({ "log_error": text })
    }
}

/// Convenience: an `Events` impl backed by a closure receiving the `Value`
/// payload. Tauri uses this to forward every event to `app.emit(...)`.
pub struct FnEvents<F: Fn(Value) + Send + Sync>(pub F);

impl<F: Fn(Value) + Send + Sync> Events for FnEvents<F> {
    fn step(&self, text: &str) {
        (self.0)(payload::step(text));
    }
    fn step_start(&self, goal: &str, id: u32) {
        (self.0)(payload::step_start(goal, id));
    }
    fn step_done(&self, id: u32, success: bool, error: Option<&str>) {
        (self.0)(payload::step_done(id, success, error));
    }
    fn ask_human(&self, question: &str) {
        (self.0)(payload::ask_human(question));
    }
    fn done(&self, result: &str) {
        (self.0)(payload::done(result));
    }
    fn error(&self, text: &str) {
        (self.0)(payload::error(text));
    }
    fn log_error(&self, text: &str) {
        (self.0)(payload::log_error(text));
    }
}
