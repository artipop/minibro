<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const greetMsg = ref("");
const name = ref("");

async function greet() {
  // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
  greetMsg.value = await invoke("greet", { name: name.value });
}

const axTrusted = ref<boolean | null>(null);
const notificationsDump = ref("");

async function checkAx() {
  axTrusted.value = await invoke("ax_check_permission", { prompt: true });
}

async function readNotifications(expandStacks = false) {
  try {
    const items = await invoke("get_notifications", { expandStacks });
    notificationsDump.value = JSON.stringify(items, null, 2);
  } catch (e) {
    notificationsDump.value = String(e);
  }
}

interface LoggedNotification {
  seen_at_ms: number;
  description: string | null;
  texts: string[];
  identifier: string | null;
}

const watcherRunning = ref(false);
const watcherError = ref("");
const log = ref<LoggedNotification[]>([]);

async function startWatcher() {
  if (watcherRunning.value) return;
  try {
    await invoke("start_notification_watcher");
    watcherRunning.value = true;
    await listen<LoggedNotification>("notifications://new", (e) => {
      log.value.push(e.payload);
    });
    log.value = await invoke("get_notification_log");
  } catch (e) {
    watcherError.value = String(e);
  }
}

function formatTime(ms: number) {
  return new Date(ms).toLocaleTimeString();
}

const clickId = ref("");
const clickResult = ref("");

async function clickNotification(identifier: string | null) {
  if (!identifier) {
    clickResult.value = "у этого уведомления нет identifier";
    return;
  }
  try {
    await invoke("click_notification", { identifier });
    clickResult.value = `кликнул ${identifier}`;
  } catch (e) {
    clickResult.value = String(e);
  }
}
</script>

<template>
  <main class="container">
    <h1>Welcome to Tauri + Vue</h1>

    <div class="row">
      <a href="https://vite.dev" target="_blank">
        <img src="/vite.svg" class="logo vite" alt="Vite logo" />
      </a>
      <a href="https://tauri.app" target="_blank">
        <img src="/tauri.svg" class="logo tauri" alt="Tauri logo" />
      </a>
      <a href="https://vuejs.org/" target="_blank">
        <img src="./assets/vue.svg" class="logo vue" alt="Vue logo" />
      </a>
    </div>
    <p>Click on the Tauri, Vite, and Vue logos to learn more.</p>

    <form class="row" @submit.prevent="greet">
      <input id="greet-input" v-model="name" placeholder="Enter a name..." />
      <button type="submit">Greet</button>
    </form>
    <p>{{ greetMsg }}</p>

    <h2>Уведомления macOS (Accessibility)</h2>
    <div class="row">
      <button @click="checkAx">Проверить доступ</button>
      <button @click="readNotifications()">Прочитать уведомления</button>
      <button @click="readNotifications(true)">Прочитать с раскрытием стопок</button>
    </div>
    <p v-if="axTrusted !== null">
      Accessibility: {{ axTrusted ? "разрешено" : "нет доступа" }}
    </p>
    <pre v-if="notificationsDump" class="notifications-dump">{{ notificationsDump }}</pre>

    <h2>Наблюдатель (фоновый лог)</h2>
    <div class="row">
      <button @click="startWatcher" :disabled="watcherRunning">
        {{ watcherRunning ? "Наблюдатель работает" : "Запустить наблюдатель" }}
      </button>
    </div>
    <p v-if="watcherError">{{ watcherError }}</p>
    <ul v-if="log.length" class="notification-log">
      <li
        v-for="(n, i) in log"
        :key="i"
        class="log-item"
        title="Кликнуть по уведомлению"
        @click="clickNotification(n.identifier)"
      >
        <span class="log-time">{{ formatTime(n.seen_at_ms) }}</span>
        {{ n.description ?? n.texts.join(" — ") }}
      </li>
    </ul>

    <form class="row" @submit.prevent="clickNotification(clickId)">
      <input v-model="clickId" placeholder="AXIdentifier уведомления…" />
      <button type="submit">Кликнуть по ID</button>
    </form>
    <p v-if="clickResult">{{ clickResult }}</p>
  </main>
</template>

<style scoped>
.logo.vite:hover {
  filter: drop-shadow(0 0 2em #747bff);
}

.logo.vue:hover {
  filter: drop-shadow(0 0 2em #249b73);
}

</style>
<style>
:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px;
  line-height: 24px;
  font-weight: 400;

  color: #0f0f0f;
  background-color: #f6f6f6;

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

.container {
  margin: 0;
  padding-top: 10vh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  text-align: center;
}

.logo {
  height: 6em;
  padding: 1.5em;
  will-change: filter;
  transition: 0.75s;
}

.logo.tauri:hover {
  filter: drop-shadow(0 0 2em #24c8db);
}

.row {
  display: flex;
  justify-content: center;
  gap: 5px;
}

.notification-log {
  text-align: left;
  max-width: 80%;
  margin: 1em auto;
  padding-left: 1.5em;
}

.log-item {
  cursor: pointer;
}

.log-item:hover {
  text-decoration: underline;
}

.log-time {
  opacity: 0.6;
  margin-right: 0.5em;
  font-variant-numeric: tabular-nums;
}

.notifications-dump {
  text-align: left;
  max-width: 80%;
  margin: 1em auto;
  padding: 1em;
  border-radius: 8px;
  background-color: rgba(128, 128, 128, 0.15);
  overflow: auto;
  white-space: pre-wrap;
}

a {
  font-weight: 500;
  color: #646cff;
  text-decoration: inherit;
}

a:hover {
  color: #535bf2;
}

h1 {
  text-align: center;
}

input,
button {
  border-radius: 8px;
  border: 1px solid transparent;
  padding: 0.6em 1.2em;
  font-size: 1em;
  font-weight: 500;
  font-family: inherit;
  color: #0f0f0f;
  background-color: #ffffff;
  transition: border-color 0.25s;
  box-shadow: 0 2px 2px rgba(0, 0, 0, 0.2);
}

button {
  cursor: pointer;
}

button:hover {
  border-color: #396cd8;
}
button:active {
  border-color: #396cd8;
  background-color: #e8e8e8;
}

input,
button {
  outline: none;
}

#greet-input {
  margin-right: 5px;
}

@media (prefers-color-scheme: dark) {
  :root {
    color: #f6f6f6;
    background-color: #2f2f2f;
  }

  a:hover {
    color: #24c8db;
  }

  input,
  button {
    color: #ffffff;
    background-color: #0f0f0f98;
  }
  button:active {
    background-color: #0f0f0f69;
  }
}

</style>