# minibro

A Tauri (CEF) + Vue desktop app with a built-in **browser agent**. You type a
task in natural language; an LLM drives the tray browser to do it, pausing to
ask you for help (login, CAPTCHA, 2FA) when needed.

## Architecture

```
App.vue ──invoke('run_agent')──▶ src-tauri (Rust, Tauri commands)
   ▲                                   │ spawns
   └──listen('agent://event')──────────┤
                                       ▼
                            src-agent (lib crate: the loop)
            LLM (OpenAI / local MLX) ──tool calls──▶ agent-browser CLI
                                                      └──CDP :9229──▶ tray browser
```

- **`src/App.vue`** — UI. Calls the `run_agent` command and listens for
  `agent://event` progress events (`step_start`/`step_done`/`ask_human`/`done`/
  `error`). Human-in-the-loop answers go back via `hitl_reply`; `agent_stop`
  cancels.
- **`src-tauri/`** — the Tauri app. Hosts the agent in-process (no sidecar
  binary) and forwards events to the frontend. Launches the tray webview with
  `--remote-debugging-port=9229 --remote-allow-origins=*` so the agent can reach
  it over CDP.
- **`src-agent/`** — UI-agnostic agent core (`agent` crate): the LLM
  tool-calling loop, an OpenAI-compatible client (OpenAI or a local MLX server),
  and a thin driver over the [`agent-browser`](https://github.com/vercel-labs/agent-browser)
  CLI for browser control. `agent-cli` is a terminal harness for the same loop.

## Requirements

- Rust + the CEF Tauri toolchain (`pnpm`, `@tauri-apps/cli-cef`).
- [`agent-browser`](https://github.com/vercel-labs/agent-browser) on `PATH`
  (`cargo install agent-browser`). It connects to the existing tray browser over
  CDP, so no separate Chrome download is needed for the app.
- `OPENAI_API_KEY` (passed from the UI via `VITE_OPENAI_API_KEY`) for the OpenAI
  provider; `mlx_lm` installed for the local MLX provider.

## Develop

```bash
pnpm install
pnpm tauri dev
```

### Test the agent loop without the GUI

```bash
# Start any Chrome with CDP, then point the harness at it:
AGENT_BROWSER_CDP=9222 OPENAI_API_KEY=sk-… \
  cargo run -p agent --bin agent-cli -- \
  '{"command":"run_agent","task":"open example.com and report the heading","provider":"openai","model":"gpt-4o-mini"}'
```
