//! Terminal harness for the agent core — exercises the exact same loop the
//! Tauri backend runs, without needing the GUI. Mirrors the legacy Python
//! sidecar protocol so it can be driven the same way:
//!
//!   agent-cli '{"command":"run_agent","task":"…","provider":"openai",
//!               "model":"gpt-4o-mini","api_key":"sk-…","cdp_port":9229}'
//!
//! Emits NDJSON on stdout; reads one stdin line per `ask_human`.
//! Point it at a plain Chrome with `AGENT_BROWSER_CDP=<port|ws>` for testing.

use agent::{run_agent, Config, FnEvents};
use serde::Deserialize;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Req {
    #[serde(default)]
    command: String,
    #[serde(default)]
    task: String,
    #[serde(default = "default_provider")]
    provider: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default = "default_cdp_port")]
    cdp_port: u16,
}

fn default_provider() -> String {
    "openai".into()
}
fn default_model() -> String {
    "gpt-4o-mini".into()
}
fn default_cdp_port() -> u16 {
    9229
}

fn emit(v: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

#[tokio::main]
async fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        emit(serde_json::json!({ "error": "argv[1] missing" }));
        return;
    };
    let req: Req = match serde_json::from_str(&arg) {
        Ok(r) => r,
        Err(e) => {
            emit(serde_json::json!({ "error": format!("bad JSON: {e}") }));
            return;
        }
    };
    if req.command != "run_agent" {
        emit(serde_json::json!({ "error": format!("unknown command: {}", req.command) }));
        return;
    }
    if req.task.trim().is_empty() {
        emit(serde_json::json!({ "error": "task is empty" }));
        return;
    }

    let events = Arc::new(FnEvents(emit));
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    // Feed stdin lines to the HITL channel.
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let cfg = Config {
        task: req.task,
        provider: req.provider,
        model: req.model,
        api_key: req.api_key,
        cdp_port: req.cdp_port,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    run_agent(cfg, events, rx, cancel).await;
}
