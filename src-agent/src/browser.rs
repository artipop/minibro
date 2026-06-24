//! Thin async wrapper over the `agent-browser` CLI (vercel-labs/agent-browser).
//!
//! `agent-browser` runs a persistent per-session daemon and speaks Chrome
//! DevTools Protocol. We connect it to minibro's tray webview (CDP on the given
//! port), locking onto the tray page target — not the `tauri://` chrome or the
//! `localhost:1420` main UI. Every command is one short-lived CLI invocation
//! that talks to the daemon; we pass `--session` + `--cdp` each time so a
//! dropped daemon transparently reconnects.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

pub struct Browser {
    /// Resolved path to the `agent-browser` executable.
    bin: PathBuf,
    /// Isolated daemon session name (unique per agent run).
    session: String,
    /// Value for `--cdp`: a `ws://…/devtools/page/<id>` URL locked to the tray
    /// target, or a bare port if no specific target was resolved.
    cdp: String,
}

impl Browser {
    /// Connect to the tray browser on `cdp_port`, locking onto its page target.
    /// `AGENT_BROWSER_CDP` overrides the `--cdp` value (handy for testing
    /// against a plain Chrome launched with `--remote-debugging-port`).
    pub async fn connect(cdp_port: u16) -> Result<Self, String> {
        let bin = resolve_agent_browser()
            .ok_or_else(|| "agent-browser not found on PATH (install: cargo install agent-browser)".to_string())?;

        let cdp = match std::env::var("AGENT_BROWSER_CDP") {
            Ok(v) if !v.is_empty() => v,
            _ => resolve_tray_target(cdp_port)
                .await
                .unwrap_or_else(|| cdp_port.to_string()),
        };

        let session = format!("minibro-{}", std::process::id());
        Ok(Self { bin, session, cdp })
    }

    /// Run one `agent-browser` subcommand with `--json`, returning the parsed
    /// `data` value on success or the `error` string on failure.
    pub async fn run(&self, args: &[&str]) -> Result<Value, String> {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("--session")
            .arg(&self.session)
            .arg("--cdp")
            .arg(&self.cdp)
            .arg("--json")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to run agent-browser: {e}"))?;

        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout.trim();
        if line.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "agent-browser produced no output (exit {:?}): {}",
                out.status.code(),
                stderr.trim()
            ));
        }

        // agent-browser may print log lines before the JSON; take the last
        // non-empty line, which is the result object.
        let json_line = line.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or(line);
        let v: Value = serde_json::from_str(json_line)
            .map_err(|e| format!("bad agent-browser JSON: {e} — raw: {json_line}"))?;

        if v.get("success").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("data").cloned().unwrap_or(Value::Null))
        } else {
            Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown agent-browser error")
                .to_string())
        }
    }

    // ── High-level actions ─────────────────────────────────────────────────

    pub async fn navigate(&self, url: &str) -> Result<(), String> {
        self.run(&["open", url]).await.map(|_| ())
    }

    pub async fn click(&self, sel: &str) -> Result<(), String> {
        self.run(&["click", &norm_ref(sel)]).await.map(|_| ())
    }

    pub async fn fill(&self, sel: &str, text: &str) -> Result<(), String> {
        self.run(&["fill", &norm_ref(sel), text]).await.map(|_| ())
    }

    pub async fn type_text(&self, sel: &str, text: &str) -> Result<(), String> {
        self.run(&["type", &norm_ref(sel), text]).await.map(|_| ())
    }

    pub async fn press(&self, key: &str) -> Result<(), String> {
        self.run(&["press", key]).await.map(|_| ())
    }

    pub async fn scroll(&self, direction: &str, px: Option<i64>) -> Result<(), String> {
        match px {
            Some(n) => {
                let n = n.to_string();
                self.run(&["scroll", direction, &n]).await.map(|_| ())
            }
            None => self.run(&["scroll", direction]).await.map(|_| ()),
        }
    }

    pub async fn get_text(&self, sel: &str) -> Result<String, String> {
        let d = self.run(&["get", "text", &norm_ref(sel)]).await?;
        Ok(d.get("text").and_then(Value::as_str).unwrap_or("").to_string())
    }

    pub async fn eval(&self, js: &str) -> Result<Value, String> {
        let d = self.run(&["eval", js]).await?;
        Ok(d.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn wait_ms(&self, ms: u64) -> Result<(), String> {
        let ms = ms.to_string();
        self.run(&["wait", &ms]).await.map(|_| ())
    }

    pub async fn url(&self) -> Result<String, String> {
        let d = self.run(&["get", "url"]).await?;
        Ok(d.get("url").and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// Interactive accessibility snapshot (`snapshot -i`) — the ref-annotated
    /// tree the LLM reads to choose `@eN` targets.
    pub async fn snapshot(&self) -> Result<String, String> {
        let d = self.run(&["snapshot", "-i"]).await?;
        Ok(d.get("snapshot").and_then(Value::as_str).unwrap_or("").to_string())
    }
}

/// Normalize an element ref: agent-browser expects `@e1`, but the model may
/// pass a bare `e1`. CSS selectors and other values pass through untouched.
fn norm_ref(sel: &str) -> String {
    if !sel.starts_with('@')
        && sel.len() >= 2
        && sel.starts_with('e')
        && sel[1..].chars().all(|c| c.is_ascii_digit())
    {
        format!("@{sel}")
    } else {
        sel.to_string()
    }
}

/// Locate the `agent-browser` executable: PATH first, then common install dirs.
fn resolve_agent_browser() -> Option<PathBuf> {
    if let Ok(p) = which("agent-browser") {
        return Some(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/agent-browser"),
        "/opt/homebrew/bin/agent-browser".to_string(),
        "/usr/local/bin/agent-browser".to_string(),
    ];
    candidates.into_iter().map(PathBuf::from).find(|p| p.exists())
}

/// Minimal `which`: scan `$PATH` for an executable named `name`.
fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

/// Query the CDP HTTP endpoint and return the tray page target's
/// `webSocketDebuggerUrl`, skipping the `tauri://` chrome and the
/// `localhost:1420` main UI. Returns `None` if the endpoint is unreachable.
async fn resolve_tray_target(port: u16) -> Option<String> {
    let url = format!("http://localhost:{port}/json");
    let client = reqwest::Client::new();
    let targets: Vec<Value> = client
        .get(&url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let is_tray = |t: &&Value| -> bool {
        let u = t.get("url").and_then(Value::as_str).unwrap_or("");
        t.get("type").and_then(Value::as_str) == Some("page")
            && !u.starts_with("tauri://")
            && !u.contains("localhost:1420")
    };

    targets
        .iter()
        .find(is_tray)
        // fall back to any page if the tray heuristic matched nothing
        .or_else(|| targets.iter().find(|t| t.get("type").and_then(Value::as_str) == Some("page")))
        .and_then(|t| t.get("webSocketDebuggerUrl").and_then(Value::as_str))
        .map(str::to_string)
}
