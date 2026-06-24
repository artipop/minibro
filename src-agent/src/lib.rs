//! Browser-agent core for minibro.
//!
//! UI-agnostic: an OpenAI-compatible model (OpenAI or local MLX) drives a real
//! browser via the `agent-browser` CLI over CDP, with human-in-the-loop and
//! per-step progress reported through the [`Events`] trait. The Tauri backend
//! and the `agent-cli` test harness are both thin consumers of [`run_agent`].

pub mod agent;
pub mod browser;
pub mod events;
pub mod llm;

pub use agent::{run_agent, Config};
pub use events::{payload, Events, FnEvents};
