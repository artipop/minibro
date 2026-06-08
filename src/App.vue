<script setup lang="ts">
import { ref, nextTick, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Command, type Child } from "@tauri-apps/plugin-shell";

type Provider = "openai" | "mlx";

interface LogEntry {
  type: "step" | "ask_human" | "done" | "error" | "user_reply";
  text: string;
}

const task = ref("");
const provider = ref<Provider>("openai");
const model = ref("gpt-5.4-mini");
const running = ref(false);
const log = ref<LogEntry[]>([]);
const logEl = ref<HTMLElement | null>(null);

// Human-in-the-loop state
const hitlQuestion = ref("");
const hitlAnswer = ref("");
let activeChild: Child | null = null;

watch(hitlQuestion, (q) => {
  invoke("set_hitl_active", { active: !!q }).catch(() => {});
});

function addLog(entry: LogEntry) {
  log.value.push(entry);
  nextTick(() => {
    if (logEl.value) logEl.value.scrollTop = logEl.value.scrollHeight;
  });
}

const MLX_MODELS = [
  { label: "Gemma 3 4B", id: "mlx-community/gemma-3-4b-it-4bit" },
  { label: "Gemma 3 12B", id: "mlx-community/gemma-3-12b-it-4bit" },
];

function onModelPreset(p: Provider) {
  provider.value = p;
  model.value = p === "openai" ? "gpt-4o-mini" : MLX_MODELS[0].id;
}

async function runAgent() {
  if (!task.value.trim() || running.value) return;
  running.value = true;
  hitlQuestion.value = "";
  hitlAnswer.value = "";
  log.value = [];
  activeChild = null;

  const request = {
    command: "run_agent",
    task: task.value.trim(),
    provider: provider.value,
    model: model.value,
    ...(provider.value === "openai"
      ? { api_key: import.meta.env.VITE_OPENAI_API_KEY ?? "" }
      : {}),
  };

  addLog({ type: "step", text: `Spawning sidecar (${provider.value}/${model.value})…` });
  console.log("[minibro] spawn request:", request);

  let cmd: ReturnType<typeof Command.sidecar>;
  try {
    cmd = Command.sidecar("binaries/python-sidecar", [JSON.stringify(request)]);
  } catch (err) {
    addLog({ type: "error", text: `Failed to create command: ${err}` });
    running.value = false;
    return;
  }

  cmd.stdout.on("data", (line: string) => {
    line = line.trim();
    console.log("[minibro] stdout:", line);
    if (!line) return;
    try {
      const data = JSON.parse(line);
      if (data.step) {
        addLog({ type: "step", text: data.step });
      } else if (data.ask_human) {
        hitlQuestion.value = data.ask_human;
        hitlAnswer.value = "";
        addLog({ type: "ask_human", text: data.ask_human });
        invoke("show_tray_window").catch(() => {});
      } else if (data.done) {
        addLog({ type: "done", text: data.result ?? "Task completed" });
        running.value = false;
        hitlQuestion.value = "";
      } else if (data.error) {
        addLog({ type: "error", text: data.error });
        running.value = false;
        hitlQuestion.value = "";
      } else {
        addLog({ type: "step", text: `[raw] ${line}` });
      }
    } catch {
      addLog({ type: "step", text: `[raw] ${line}` });
    }
  });

  cmd.stderr.on("data", (line: string) => {
    line = line.trim();
    if (!line) return;
    console.warn("[minibro] stderr:", line);
    addLog({ type: "error", text: `[stderr] ${line}` });
  });

  cmd.on("close", (data) => {
    console.log("[minibro] process closed, code:", data?.code);
    addLog({ type: "step", text: `Process exited (code ${data?.code ?? "?"})` });
    running.value = false;
    hitlQuestion.value = "";
    activeChild = null;
  });

  cmd.on("error", (err) => {
    console.error("[minibro] process error:", err);
    addLog({ type: "error", text: `Spawn error: ${err}` });
    running.value = false;
    hitlQuestion.value = "";
    activeChild = null;
  });

  try {
    activeChild = await cmd.spawn();
    console.log("[minibro] spawned pid:", activeChild.pid);
    addLog({ type: "step", text: `Sidecar started (pid ${activeChild.pid})` });
  } catch (err) {
    console.error("[minibro] spawn failed:", err);
    addLog({ type: "error", text: `Spawn failed: ${err}` });
    running.value = false;
  }
}

async function sendHitlAnswer() {
  if (!activeChild || !hitlAnswer.value.trim()) return;
  const answer = hitlAnswer.value.trim();
  addLog({ type: "user_reply", text: answer });
  await activeChild.write(answer + "\n");
  hitlQuestion.value = "";
  hitlAnswer.value = "";
}

function stopAgent() {
  activeChild?.kill().catch(() => {});
  running.value = false;
  hitlQuestion.value = "";
}
</script>

<template>
  <main class="app">
    <h1>minibro</h1>

    <section class="config">
      <div class="row">
        <button
          :class="['preset', provider === 'openai' ? 'active' : '']"
          @click="onModelPreset('openai')"
        >OpenAI</button>
        <button
          :class="['preset', provider === 'mlx' ? 'active' : '']"
          @click="onModelPreset('mlx')"
        >MLX</button>
        <template v-if="provider === 'mlx'">
          <select v-model="model" class="model-input">
            <option v-for="m in MLX_MODELS" :key="m.id" :value="m.id">{{ m.label }}</option>
          </select>
        </template>
        <input v-else v-model="model" class="model-input" placeholder="model name" />
      </div>
    </section>

    <section class="task-section">
      <textarea
        v-model="task"
        class="task-input"
        placeholder="Describe a task for the browser agent…"
        rows="3"
        :disabled="running"
        @keydown.meta.enter="runAgent"
        @keydown.ctrl.enter="runAgent"
      />
      <div class="row">
        <button class="run-btn" @click="runAgent" :disabled="running || !task.trim()">
          {{ running ? "Running…" : "Run Agent" }}
        </button>
        <button v-if="running" class="stop-btn" @click="stopAgent">Stop</button>
      </div>
    </section>

    <!-- Human-in-the-loop panel -->
    <section v-if="hitlQuestion" class="hitl-panel">
      <p class="hitl-question">{{ hitlQuestion }}</p>
      <div class="row">
        <input
          v-model="hitlAnswer"
          class="hitl-input"
          placeholder="Type a reply, or just press Send when done…"
          @keydown.enter="sendHitlAnswer"
        />
        <button class="send-btn" @click="sendHitlAnswer">Send</button>
      </div>
    </section>

    <!-- Event log -->
    <section v-if="log.length" class="log-section">
      <div ref="logEl" class="log">
        <div v-for="(entry, i) in log" :key="i" :class="['log-entry', entry.type]">
          <span class="badge">{{ entry.type }}</span>
          <span class="text">{{ entry.text }}</span>
        </div>
      </div>
    </section>
  </main>
</template>

<style scoped>
.app {
  max-width: 640px;
  margin: 0 auto;
  padding: 2rem 1.5rem;
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
}

h1 {
  font-size: 1.4rem;
  font-weight: 600;
  margin: 0;
}

.config .row {
  display: flex;
  gap: 0.5rem;
  align-items: center;
}

.preset {
  padding: 0.35rem 0.85rem;
  border-radius: 6px;
  border: 1px solid #555;
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 0.85rem;
}
.preset.active {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}

.model-input {
  flex: 1;
  padding: 0.35rem 0.65rem;
  border-radius: 6px;
  border: 1px solid #555;
  background: transparent;
  color: inherit;
  font-size: 0.85rem;
}

.task-section {
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}

.task-input {
  width: 100%;
  resize: vertical;
  padding: 0.65rem;
  border-radius: 8px;
  border: 1px solid #555;
  background: transparent;
  color: inherit;
  font-family: inherit;
  font-size: 0.95rem;
  line-height: 1.5;
  box-sizing: border-box;
}

.row {
  display: flex;
  gap: 0.5rem;
  align-items: center;
}

.run-btn {
  padding: 0.5rem 1.2rem;
  border-radius: 8px;
  border: none;
  background: #396cd8;
  color: #fff;
  font-weight: 600;
  cursor: pointer;
}
.run-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.stop-btn {
  padding: 0.5rem 1rem;
  border-radius: 8px;
  border: 1px solid #c0392b;
  background: transparent;
  color: #c0392b;
  cursor: pointer;
}

.hitl-panel {
  border: 1px solid #f39c12;
  border-radius: 10px;
  padding: 1rem;
  background: rgba(243, 156, 18, 0.07);
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.hitl-question {
  margin: 0;
  font-weight: 500;
  color: #f39c12;
}

.hitl-input {
  flex: 1;
  padding: 0.5rem 0.75rem;
  border-radius: 8px;
  border: 1px solid #555;
  background: transparent;
  color: inherit;
  font-size: 0.9rem;
}

.send-btn {
  padding: 0.5rem 1rem;
  border-radius: 8px;
  border: none;
  background: #f39c12;
  color: #000;
  font-weight: 600;
  cursor: pointer;
}

.log-section {
  display: flex;
  flex-direction: column;
}

.log {
  max-height: 320px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  padding: 0.75rem;
  background: #111;
  border-radius: 10px;
  font-size: 0.85rem;
}

.log-entry {
  display: flex;
  gap: 0.6rem;
  align-items: flex-start;
}

.badge {
  flex-shrink: 0;
  font-size: 0.7rem;
  font-weight: 700;
  padding: 0.1rem 0.45rem;
  border-radius: 4px;
  text-transform: uppercase;
  margin-top: 1px;
}

.log-entry.step .badge        { background: #2c3e50; color: #95a5a6; }
.log-entry.ask_human .badge   { background: #7d6608; color: #f1c40f; }
.log-entry.done .badge        { background: #1a5c2a; color: #2ecc71; }
.log-entry.error .badge       { background: #5c1a1a; color: #e74c3c; }
.log-entry.user_reply .badge  { background: #1a3a5c; color: #3498db; }

.text {
  color: #ccc;
  line-height: 1.5;
  word-break: break-word;
}
</style>

<style>
* { box-sizing: border-box; }

:root {
  font-family: Inter, system-ui, sans-serif;
  font-size: 16px;
  color: #f0f0f0;
  background: #1a1a1a;
  -webkit-font-smoothing: antialiased;
}

body { margin: 0; }
input, textarea, button { font-family: inherit; outline: none; }
button { transition: opacity 0.15s; }
button:hover:not(:disabled) { opacity: 0.85; }
</style>
