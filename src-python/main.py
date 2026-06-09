#!/usr/bin/env python3
"""
Browser-use agent sidecar for minibro (browser-use >= 0.12).

Protocol
--------
argv[1]  JSON: {"command": "run_agent", "task": "...",
                "provider": "openai"|"mlx",
                "model":    "gpt-4o-mini" | "mlx-community/gemma-3-4b-it-4bit",
                "api_key":  "sk-..."  (openai only)}

stdout   NDJSON (one JSON object per line):
  {"step": "..."}               — progress / agent next-goal
  {"ask_human": "question"}     — agent pauses, waits for one stdin line
  {"done": true, "result": "…"} — finished
  {"error": "…"}                — fatal error

stdin    one text line per ask_human answer (Vue calls child.write)
"""
import asyncio
import json
import logging
import os
import socket
import sys

os.environ.setdefault("ANONYMIZED_TELEMETRY", "false")
os.environ.setdefault("BROWSER_USE_SETUP_LOGGING", "false")
os.environ.setdefault("TIMEOUT_ScreenshotEvent", "60")


class _StdoutErrorHandler(logging.Handler):
    """Route ERROR+ log records to stdout as structured events; drop the rest."""
    def emit(self, record: logging.LogRecord) -> None:
        if record.levelno >= logging.ERROR:
            try:
                emit({"log_error": self.format(record)})
            except Exception:
                pass


logging.disable(logging.WARNING)          # drop DEBUG / INFO / WARNING
_handler = _StdoutErrorHandler()
_handler.setLevel(logging.ERROR)
logging.root.addHandler(_handler)         # still catches ERROR / CRITICAL


# ── helpers ──────────────────────────────────────────────────────────────────

def emit(data: dict) -> None:
    print(json.dumps(data, ensure_ascii=False), flush=True)


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


# ── MLX local server ──────────────────────────────────────────────────────────

async def start_mlx_server(model: str) -> tuple[asyncio.subprocess.Process, int]:
    """Start mlx_lm.server on a free port; return (process, port)."""
    import httpx

    port = free_port()
    emit({"step": f"Starting MLX server ({model})…"})

    proc = await asyncio.create_subprocess_exec(
        sys.executable, "-m", "mlx_lm.server",
        "--model", model,
        "--port", str(port),
        "--host", "127.0.0.1",
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.DEVNULL,
    )

    # Poll until server responds (model may need to download — allow 10 min)
    health = f"http://127.0.0.1:{port}/v1/models"
    for elapsed in range(600):
        await asyncio.sleep(1)
        if proc.returncode is not None:
            raise RuntimeError(f"MLX server exited early (code {proc.returncode})")
        try:
            async with httpx.AsyncClient() as client:
                r = await client.get(health, timeout=2.0)
                if r.status_code == 200:
                    emit({"step": "MLX model loaded, agent starting…"})
                    return proc, port
        except Exception:
            pass
        if elapsed > 0 and elapsed % 15 == 0:
            emit({"step": f"Loading model… ({elapsed}s, first run downloads from HuggingFace)"})

    proc.kill()
    raise RuntimeError("MLX server did not start within 10 minutes")


# ── LLM factory ──────────────────────────────────────────────────────────────

def make_llm(provider: str, model: str, api_key: str, port: int | None = None):
    if provider == "mlx":
        from browser_use.llm.openai.like import ChatOpenAILike
        return ChatOpenAILike(
            model="local",                            # mlx-lm ignores the name
            base_url=f"http://127.0.0.1:{port}/v1",
            api_key="x",                              # required by openai client, value ignored
        )
    else:
        from browser_use import ChatOpenAI
        return ChatOpenAI(model=model, api_key=api_key or None)


# ── agent ─────────────────────────────────────────────────────────────────────

async def find_tray_target_id(session) -> str | None:
    """Return target_id of the tray browser (not tauri:// and not localhost:1420)."""
    targets = session.session_manager.get_all_page_targets()
    for t in targets:
        url = t.url or ""
        if not url.startswith("tauri://") and "localhost:1420" not in url:
            return t.target_id
    return None


async def run_agent(task: str, api_key: str, provider: str, model: str) -> None:
    from browser_use import Agent, BrowserProfile, Controller
    from browser_use.browser.session import BrowserSession

    if provider == "openai" and api_key:
        os.environ["OPENAI_API_KEY"] = api_key

    mlx_proc: asyncio.subprocess.Process | None = None
    mlx_port: int | None = None
    browser_session: BrowserSession | None = None
    agent_ref: list = [None]
    last_step_reported = [0]

    try:
        if provider == "mlx":
            mlx_proc, mlx_port = await start_mlx_server(model)

        controller = Controller()

        @controller.action(
            "Ask the human user to do something that requires manual interaction: "
            "log in, solve a CAPTCHA, confirm a dialog, handle 2FA, dismiss a "
            "large cookie/GDPR consent banner that covers significant page area "
            "and blocks you from clicking elements, or any other step you cannot "
            "complete yourself. IMPORTANT: call this action as soon as you "
            "encounter any of these blockers. Do NOT retry the same failing "
            "action more than twice — ask the human instead. The human will act "
            "in the visible browser window and reply when done."
        )
        async def ask_human(question: str) -> str:
            emit({"ask_human": question})
            emit({"step": f"[HITL] Waiting for human: {question[:100]}"})
            try:
                loop = asyncio.get_running_loop()
                raw = await loop.run_in_executor(None, sys.stdin.readline)
                stripped = raw.strip()
                emit({"step": f"[HITL] Received raw stdin: {repr(raw[:120])}"})
                if not stripped:
                    stripped = "done"
                emit({"step": f"[HITL] Returning to agent: {stripped[:100]}"})
                # Wrap the answer so the LLM clearly understands the human acted and why.
                return (
                    f"The human has completed the requested action in the browser. "
                    f"Human's message: \"{stripped}\". "
                    f"The page state may have changed — take a screenshot and continue the task."
                )
            except Exception as exc:
                emit({"step": f"[HITL] stdin error: {exc}"})
                return f"(stdin error: {exc})"

        def on_step(browser_state, agent_output, step_num: int) -> None:
            goal = getattr(agent_output, "next_goal", None) or f"step {step_num}"

            # When step N starts, step N-1 just completed — report its result.
            prev = step_num - 1
            if prev > 0 and prev > last_step_reported[0] and agent_ref[0] is not None:
                try:
                    h = agent_ref[0].history.history
                    idx = prev - 1
                    if 0 <= idx < len(h):
                        results = getattr(h[idx], "result", None) or []
                        errors = [r.error for r in results if getattr(r, "error", None)]
                        # Log the raw action results for debugging HITL re-ask issues.
                        for r in results:
                            extracted = getattr(r, "extracted_content", None)
                            if extracted:
                                emit({"step": f"[debug] step {prev} result: {str(extracted)[:200]}"})
                        emit({"step_done": {
                            "id": prev,
                            "success": not errors,
                            "error": errors[0] if errors else None,
                        }})
                        last_step_reported[0] = prev
                except Exception as exc:
                    emit({"step": f"[debug] step_done extraction error: {exc}"})

            # Log current URL and model evaluation for post-HITL debugging.
            try:
                url = getattr(browser_state, "url", None) or ""
                if url:
                    emit({"step": f"[debug] url: {url[:120]}"})
            except Exception:
                pass
            try:
                evaluation = getattr(
                    getattr(agent_output, "current_state", None),
                    "evaluation_previous_goal", None
                )
                if evaluation:
                    emit({"step": f"[debug] eval: {str(evaluation)[:200]}"})
            except Exception:
                pass

            emit({"step_start": goal, "step_id": step_num})

        # Verify CDP is up before handing to browser-use; if we pass a bad URL,
        # browser-use falls back to launching its own Chromium (dock bounce on macOS).
        import httpx
        cdp_url = "http://localhost:9229"
        for attempt in range(10):
            try:
                async with httpx.AsyncClient() as c:
                    r = await c.get(f"{cdp_url}/json/version", timeout=2.0)
                    if r.status_code == 200:
                        break
            except Exception:
                pass
            if attempt == 9:
                emit({"error": "Tray browser CDP not reachable on port 9229"})
                return
            await asyncio.sleep(0.5)
            emit({"step": f"Waiting for tray browser… ({attempt + 1}/10)"})

        profile = BrowserProfile(
            cdp_url=cdp_url,
            minimum_wait_page_load_time=0.1,
            wait_between_actions=0.1,
        )
        llm = make_llm(provider, model, api_key, mlx_port)

        # Start session manually so we can lock focus to the tray browser target
        # before the agent sees multiple tabs and picks the wrong one.
        browser_session = BrowserSession(browser_profile=profile)
        await browser_session.start()

        tray_id = await find_tray_target_id(browser_session)
        if tray_id:
            browser_session.agent_focus_target_id = tray_id
            emit({"step": f"Locked to tray browser target {tray_id[:8]}…"})
        else:
            emit({"step": "Warning: could not identify tray browser target, using first available"})

        agent = Agent(
            task=task,
            llm=llm,
            browser_session=browser_session,
            controller=controller,
            register_new_step_callback=on_step,
            generate_gif=False,
            use_judge=False,
            flash_mode=True,
            max_failures=3,
            extend_system_message=(
                "When you cannot proceed — login required, CAPTCHA, 2FA, "
                "permission denied, or any blocker — STOP retrying and call "
                "ask_human() immediately. Do not attempt the same failing "
                "action more than twice. "
                "If a cookie consent or GDPR banner covers a large portion of "
                "the page and prevents you from interacting with elements behind "
                "it, call ask_human() to let the human dismiss it. "
                "Always respond in the same language the user used in their task. "
                "Be extremely concise and direct. "
                "Use multi-action sequences whenever possible to reduce steps."
            ),
        )
        agent_ref[0] = agent

        emit({"step": f"Agent running ({provider}/{model})"})
        history = await agent.run(max_steps=50)

        # Report result of the final step (no subsequent step fires for it).
        try:
            h = history.history
            if h:
                final_step = len(h)
                if final_step > last_step_reported[0]:
                    idx = final_step - 1
                    results = getattr(h[idx], "result", None) or []
                    errors = [r.error for r in results if getattr(r, "error", None)]
                    emit({"step_done": {
                        "id": final_step,
                        "success": not errors,
                        "error": errors[0] if errors else None,
                    }})
        except Exception:
            pass

        result = getattr(history, "final_result", lambda: None)()
        emit({"done": True, "result": result or "Task completed"})

    except Exception as exc:
        emit({"error": str(exc)})
    finally:
        if browser_session is not None:
            try:
                await browser_session.stop()
            except Exception:
                pass
        if mlx_proc is not None:
            try:
                mlx_proc.terminate()
                await asyncio.wait_for(mlx_proc.wait(), timeout=5)
            except Exception:
                mlx_proc.kill()


# ── entry point ───────────────────────────────────────────────────────────────

def main() -> None:
    if len(sys.argv) < 2:
        emit({"error": "argv[1] missing"})
        return

    try:
        request = json.loads(sys.argv[1])
    except json.JSONDecodeError as exc:
        emit({"error": f"Bad JSON: {exc}"})
        return

    command = request.get("command", "")

    if command == "run_agent":
        task    = request.get("task", "").strip()
        api_key = request.get("api_key") or os.environ.get("OPENAI_API_KEY", "")
        provider = request.get("provider", "openai")
        model   = request.get("model", "gpt-4o-mini")

        if not task:
            emit({"error": "task is empty"}); return
        if provider == "openai" and not api_key:
            emit({"error": "api_key required for openai"}); return

        asyncio.run(run_agent(task, api_key, provider, model))
    else:
        emit({"error": f"Unknown command: {command!r}"})


if __name__ == "__main__":
    main()
