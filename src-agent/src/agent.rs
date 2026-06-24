//! The browser-agent loop: an OpenAI-compatible tool-calling model drives the
//! browser through `Browser` actions, with human-in-the-loop and per-step
//! progress reporting. This is the Rust equivalent of the old Python
//! browser-use sidecar, preserving the same external behavior.

use crate::browser::Browser;
use crate::events::Events;
use crate::llm::{LlmClient, MlxServer};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

const MAX_STEPS: u32 = 50;

pub struct Config {
    pub task: String,
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub cdp_port: u16,
}

/// Run one agent task to completion. Streams events through `events`; reads
/// one line from `hitl_rx` whenever the agent asks a human; aborts promptly if
/// `cancel` flips to true.
pub async fn run_agent(
    cfg: Config,
    events: Arc<dyn Events>,
    mut hitl_rx: mpsc::UnboundedReceiver<String>,
    cancel: Arc<AtomicBool>,
) {
    if let Err(e) = run_inner(cfg, &events, &mut hitl_rx, &cancel).await {
        events.error(&e);
    }
}

async fn run_inner(
    cfg: Config,
    events: &Arc<dyn Events>,
    hitl_rx: &mut mpsc::UnboundedReceiver<String>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    // Keep the MLX server alive for the duration of the run (dropped on return).
    let _mlx_guard;
    let llm = match cfg.provider.as_str() {
        "mlx" => {
            let ev = events.clone();
            let server = MlxServer::start(&cfg.model, |m| ev.step(m)).await?;
            let base = server.base_url.clone();
            _mlx_guard = Some(server);
            LlmClient::with_base_url(base, "x".into(), "local".into())
        }
        _ => {
            let key = cfg
                .api_key
                .clone()
                .filter(|k| !k.is_empty())
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .ok_or("api_key required for openai")?;
            _mlx_guard = None;
            LlmClient::openai(key, cfg.model.clone())
        }
    };

    events.step(&format!("Connecting to tray browser (CDP {})…", cfg.cdp_port));
    let browser = Browser::connect(cfg.cdp_port).await?;
    events.step(&format!("Agent running ({}/{})", cfg.provider, cfg.model));

    let mut messages = vec![
        json!({ "role": "system", "content": system_prompt() }),
        json!({ "role": "user", "content": cfg.task }),
    ];
    let tools = tool_specs();
    let mut step_id: u32 = 0;

    for _ in 0..MAX_STEPS {
        if cancel.load(Ordering::Relaxed) {
            events.step("Stopped by user");
            return Ok(());
        }

        let message = llm.chat(&Value::Array(messages.clone()), &tools).await?;
        messages.push(message.clone());

        let tool_calls = message.get("tool_calls").and_then(Value::as_array).cloned();
        let Some(tool_calls) = tool_calls.filter(|t| !t.is_empty()) else {
            // No tool calls — treat the assistant content as the final answer.
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("Task completed");
            events.done(content);
            return Ok(());
        };

        for tc in tool_calls {
            if cancel.load(Ordering::Relaxed) {
                events.step("Stopped by user");
                return Ok(());
            }

            let call_id = tc.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args: Value = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Object(Default::default()));

            step_id += 1;
            events.step_start(&describe(&name, &args), step_id);

            // `done` and `ask_human` are handled here; everything else is a
            // browser action.
            if name == "done" {
                let result = args
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("Task completed");
                events.step_done(step_id, true, None);
                events.done(result);
                return Ok(());
            }

            if name == "ask_human" {
                let question = args
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("(no question)");
                events.ask_human(question);
                events.step(&format!("[HITL] Waiting for human: {}", truncate(question, 100)));
                let answer = hitl_rx.recv().await.unwrap_or_else(|| "done".into());
                let answer = if answer.trim().is_empty() { "done".into() } else { answer };
                events.step(&format!("[HITL] Received: {}", truncate(&answer, 100)));
                events.step_done(step_id, true, None);
                let content = format!(
                    "The human has completed the requested action in the browser. \
                     Human's message: \"{}\". The page state may have changed — take a \
                     fresh snapshot and continue the task.",
                    answer.trim()
                );
                messages.push(tool_message(&call_id, &content));
                continue;
            }

            let (content, success, error) = match execute(&browser, &name, &args).await {
                Ok(text) => (text, true, None),
                Err(e) => (format!("ERROR: {e}"), false, Some(e)),
            };
            events.step_done(step_id, success, error.as_deref());
            messages.push(tool_message(&call_id, &content));
        }
    }

    events.done("Reached step limit without an explicit finish");
    Ok(())
}

/// Execute one browser tool call, returning the string fed back to the model.
/// Mutating/navigation actions return a fresh interactive snapshot + URL so the
/// model always sees current `@eN` refs.
async fn execute(browser: &Browser, name: &str, args: &Value) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();

    match name {
        "navigate" => {
            browser.navigate(&s("url")).await?;
            Ok(with_state(browser).await)
        }
        "click" => {
            browser.click(&s("ref")).await?;
            Ok(with_state(browser).await)
        }
        "fill" => {
            browser.fill(&s("ref"), &s("text")).await?;
            Ok(with_state(browser).await)
        }
        "type" => {
            browser.type_text(&s("ref"), &s("text")).await?;
            Ok(with_state(browser).await)
        }
        "press" => {
            browser.press(&s("key")).await?;
            Ok(with_state(browser).await)
        }
        "scroll" => {
            let px = args.get("px").and_then(Value::as_i64);
            let dir = {
                let d = s("direction");
                if d.is_empty() { "down".to_string() } else { d }
            };
            browser.scroll(&dir, px).await?;
            Ok(with_state(browser).await)
        }
        "wait" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(1000);
            browser.wait_ms(ms).await?;
            Ok(with_state(browser).await)
        }
        "snapshot" => Ok(with_state(browser).await),
        "get_text" => browser.get_text(&s("ref")).await,
        "eval" => {
            let v = browser.eval(&s("js")).await?;
            Ok(v.to_string())
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// "URL: …\n\n<interactive snapshot>" — the post-action page state.
async fn with_state(browser: &Browser) -> String {
    let url = browser.url().await.unwrap_or_default();
    let snap = match browser.snapshot().await {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => "(no interactive elements)".into(),
        Err(e) => format!("(snapshot error: {e})"),
    };
    format!("URL: {url}\n\n{snap}")
}

fn tool_message(call_id: &str, content: &str) -> Value {
    json!({ "role": "tool", "tool_call_id": call_id, "content": content })
}

/// Short human-readable label for the step log.
fn describe(name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    match name {
        "navigate" => format!("Open {}", s("url")),
        "click" => format!("Click {}", s("ref")),
        "fill" => format!("Fill {} = {:?}", s("ref"), truncate(s("text"), 40)),
        "type" => format!("Type into {} = {:?}", s("ref"), truncate(s("text"), 40)),
        "press" => format!("Press {}", s("key")),
        "scroll" => format!("Scroll {}", {
            let d = s("direction");
            if d.is_empty() { "down" } else { d }
        }),
        "wait" => "Wait".to_string(),
        "snapshot" => "Read page".to_string(),
        "get_text" => format!("Get text {}", s("ref")),
        "eval" => "Run JS".to_string(),
        "ask_human" => "Ask human".to_string(),
        "done" => "Finish".to_string(),
        other => other.to_string(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn system_prompt() -> &'static str {
    "You are a browser automation agent. You control a real browser through \
     tools. Each browser action returns the current page URL and an \
     accessibility snapshot whose interactive elements are tagged with refs \
     like [ref=e1]. To act on an element pass its ref prefixed with '@', e.g. \
     click @e1, fill @e2. Always base your next action on the latest snapshot; \
     call `snapshot` if you need a fresh view.\n\n\
     When you cannot proceed — login required, CAPTCHA, 2FA, permission \
     denied, or any blocker — STOP retrying and call `ask_human` immediately. \
     Do not attempt the same failing action more than twice. If a cookie/GDPR \
     consent banner covers a large part of the page and blocks you, call \
     `ask_human` to let the human dismiss it.\n\n\
     Call `done` with a short result when the task is complete. Always respond \
     in the same language the user used in their task. Be concise and direct."
}

/// OpenAI function-tool definitions exposed to the model.
fn tool_specs() -> Value {
    let f = |name: &str, description: &str, params: Value| {
        json!({ "type": "function", "function": {
            "name": name, "description": description, "parameters": params
        }})
    };
    let obj = |props: Value, required: Value| {
        json!({ "type": "object", "properties": props, "required": required })
    };
    let str_prop = |desc: &str| json!({ "type": "string", "description": desc });

    json!([
        f("navigate", "Navigate to a URL", obj(json!({ "url": str_prop("Absolute URL to open") }), json!(["url"]))),
        f("click", "Click an element by its snapshot ref (e.g. @e1)", obj(json!({ "ref": str_prop("Element ref like @e1") }), json!(["ref"]))),
        f("fill", "Clear a field and type text into it", obj(json!({ "ref": str_prop("Element ref like @e2"), "text": str_prop("Text to enter") }), json!(["ref", "text"]))),
        f("type", "Type text into an element without clearing it", obj(json!({ "ref": str_prop("Element ref"), "text": str_prop("Text to type") }), json!(["ref", "text"]))),
        f("press", "Press a key or chord (Enter, Tab, Control+a)", obj(json!({ "key": str_prop("Key to press") }), json!(["key"]))),
        f("scroll", "Scroll the page", obj(json!({ "direction": json!({ "type": "string", "enum": ["up", "down", "left", "right"] }), "px": json!({ "type": "integer", "description": "Pixels to scroll (optional)" }) }), json!(["direction"]))),
        f("wait", "Wait for some milliseconds (e.g. for a page to settle)", obj(json!({ "ms": json!({ "type": "integer", "description": "Milliseconds to wait" }) }), json!(["ms"]))),
        f("snapshot", "Get a fresh interactive accessibility snapshot of the page", obj(json!({}), json!([]))),
        f("get_text", "Get the text content of an element", obj(json!({ "ref": str_prop("Element ref") }), json!(["ref"]))),
        f("eval", "Run JavaScript in the page and return the result", obj(json!({ "js": str_prop("JavaScript expression") }), json!(["js"]))),
        f("ask_human", "Ask the human to perform a manual step (login, CAPTCHA, 2FA, dismiss a blocking banner) in the visible browser. Call this as soon as you hit such a blocker.", obj(json!({ "question": str_prop("What you need the human to do") }), json!(["question"]))),
        f("done", "Finish the task and report the result", obj(json!({ "result": str_prop("Short summary of the outcome") }), json!(["result"]))),
    ])
}
