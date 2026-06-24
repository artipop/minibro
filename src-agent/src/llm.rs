//! OpenAI-compatible chat-completions client (works for OpenAI and for a local
//! MLX server, which speaks the same protocol on a different base URL).

use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn openai(api_key: String, model: String) -> Self {
        Self::with_base_url("https://api.openai.com/v1".into(), api_key, model)
    }

    pub fn with_base_url(base_url: String, api_key: String, model: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .expect("reqwest client");
        Self { http, base_url, api_key, model }
    }

    /// One chat-completions round. Returns the assistant `message` object
    /// (which may contain `tool_calls`).
    pub async fn chat(&self, messages: &Value, tools: &Value) -> Result<Value, String> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "temperature": 0,
        });

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("LLM request failed: {e}"))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("LLM read failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("LLM HTTP {status}: {}", text.chars().take(500).collect::<String>()));
        }

        let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad LLM JSON: {e}"))?;
        v.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| format!("LLM response missing choices[0].message: {text}"))
    }
}

/// A local `mlx_lm.server` process; killed on drop.
pub struct MlxServer {
    child: Child,
    pub base_url: String,
}

impl MlxServer {
    /// Start `python3 -m mlx_lm.server` on a free port and wait until it
    /// answers `/v1/models`. The first run may download the model from
    /// HuggingFace, so we allow up to 10 minutes.
    pub async fn start(
        model: &str,
        on_progress: impl Fn(&str),
    ) -> Result<Self, String> {
        let port = free_port()?;
        on_progress(&format!("Starting MLX server ({model})…"));

        let child = Command::new("python3")
            .args([
                "-m",
                "mlx_lm.server",
                "--model",
                model,
                "--port",
                &port.to_string(),
                "--host",
                "127.0.0.1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start mlx_lm.server (is mlx_lm installed?): {e}"))?;

        let base_url = format!("http://127.0.0.1:{port}/v1");
        let health = format!("{base_url}/models");
        let client = reqwest::Client::new();

        for elapsed in 0..600 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(r) = client.get(&health).timeout(Duration::from_secs(2)).send().await {
                if r.status().is_success() {
                    on_progress("MLX model loaded, agent starting…");
                    return Ok(Self { child, base_url });
                }
            }
            if elapsed > 0 && elapsed % 15 == 0 {
                on_progress(&format!(
                    "Loading model… ({elapsed}s, first run downloads from HuggingFace)"
                ));
            }
        }

        Err("MLX server did not start within 10 minutes".into())
    }
}

impl Drop for MlxServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Bind to port 0 to let the OS pick a free port, then release it.
fn free_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("no local addr: {e}"))?
        .port();
    Ok(port)
}
