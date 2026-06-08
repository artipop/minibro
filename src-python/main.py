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
logging.disable(logging.WARNING)


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

async def run_agent(task: str, api_key: str, provider: str, model: str) -> None:
    from browser_use import Agent, BrowserProfile, Controller

    if provider == "openai" and api_key:
        os.environ["OPENAI_API_KEY"] = api_key

    mlx_proc: asyncio.subprocess.Process | None = None
    mlx_port: int | None = None

    try:
        if provider == "mlx":
            mlx_proc, mlx_port = await start_mlx_server(model)

        controller = Controller()

        @controller.action(
            "Ask the human user to perform a manual action in the visible browser window "
            "(e.g. log in, solve CAPTCHA, accept a dialog). "
            "Use this whenever you cannot proceed without user intervention."
        )
        async def ask_human(question: str) -> str:
            emit({"ask_human": question})
            if sys.stdin is None:
                return "stdin not available"
            loop = asyncio.get_event_loop()
            answer = await loop.run_in_executor(None, sys.stdin.readline)
            return answer.strip() or "done"

        def on_step(browser_state, agent_output, step_num: int) -> None:
            goal = getattr(agent_output, "next_goal", None) or f"step {step_num}"
            emit({"step": goal})

        profile = BrowserProfile(cdp_url="http://localhost:9229")
        llm = make_llm(provider, model, api_key, mlx_port)

        agent = Agent(
            task=task,
            llm=llm,
            browser_profile=profile,
            controller=controller,
            register_new_step_callback=on_step,
            generate_gif=False,
            use_judge=False,
        )

        emit({"step": f"Agent running ({provider}/{model})"})
        history = await agent.run(max_steps=50)

        result = getattr(history, "final_result", lambda: None)()
        emit({"done": True, "result": result or "Task completed"})

    except Exception as exc:
        emit({"error": str(exc)})
    finally:
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
