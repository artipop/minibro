<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const navUrl = ref("https://example.com");
const evalScript = ref("document.title");
const evalResult = ref("");
const htmlResult = ref("");
const targetsResult = ref("");
const status = ref("");

async function navigate() {
  try {
    await invoke("navigate_tray", { url: navUrl.value });
    status.value = `Navigating to ${navUrl.value}…`;
  } catch (e) {
    status.value = `Error: ${e}`;
  }
}

async function cdpEval() {
  evalResult.value = "…";
  try {
    const r = await invoke<string>("cdp_eval", { script: evalScript.value });
    evalResult.value = r;
    status.value = "ok";
  } catch (e) {
    evalResult.value = `${e}`;
    status.value = "cdp_eval failed";
  }
}

async function getHtml() {
  htmlResult.value = "…";
  try {
    const html = await invoke<string>("cdp_get_html");
    htmlResult.value = html.slice(0, 600) + (html.length > 600 ? "…" : "");
    status.value = `HTML: ${html.length} bytes`;
  } catch (e) {
    htmlResult.value = `${e}`;
  }
}

async function listTargets() {
  try {
    targetsResult.value = await invoke<string>("cdp_list_targets");
  } catch (e) {
    targetsResult.value = `${e}`;
  }
}
</script>

<template>
  <main class="container">
    <h1>minibro</h1>

    <section>
      <h3>Navigate (background window)</h3>
      <p class="hint">Window created hidden at startup — no tray click needed</p>
      <form class="row" @submit.prevent="navigate">
        <input v-model="navUrl" placeholder="https://…" />
        <button type="submit">Go</button>
      </form>
    </section>

    <section>
      <h3>CDP eval → result</h3>
      <div class="row">
        <input v-model="evalScript" />
        <button @click="cdpEval">Run</button>
      </div>
      <pre v-if="evalResult" class="result">{{ evalResult }}</pre>
    </section>

    <section>
      <h3>Page HTML / targets</h3>
      <div class="row">
        <button @click="getHtml">Get HTML</button>
        <button @click="listTargets">List CDP targets</button>
      </div>
      <pre v-if="htmlResult" class="result">{{ htmlResult }}</pre>
      <pre v-if="targetsResult" class="result">{{ targetsResult }}</pre>
    </section>

    <p class="status">{{ status }}</p>
  </main>
</template>

<style scoped>
section { width: 100%; max-width: 560px; margin-bottom: 18px; text-align: left; }
h3 { margin: 0 0 6px; font-size: 1em; }
.hint { font-size: 0.82em; color: #888; margin: 2px 0 6px; }
.status { font-size: 0.9em; color: #396cd8; min-height: 1.4em; font-weight: 500; }
.result {
  background: #1e1e1e; color: #d4d4d4; padding: 8px; border-radius: 6px;
  font-size: 0.8em; white-space: pre-wrap; margin-top: 6px;
  max-height: 200px; overflow-y: auto;
}
</style>
<style>
:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px; line-height: 24px; font-weight: 400;
  color: #0f0f0f; background-color: #f6f6f6;
  -webkit-font-smoothing: antialiased;
}
.container { margin: 0 auto; padding: 20px 24px; display: flex; flex-direction: column; align-items: center; max-width: 600px; }
.row { display: flex; gap: 8px; flex-wrap: wrap; }
input, button {
  border-radius: 8px; border: 1px solid transparent;
  padding: 0.55em 1.1em; font-size: 1em; font-weight: 500; font-family: inherit;
  color: #0f0f0f; background-color: #fff;
  box-shadow: 0 2px 2px rgba(0,0,0,.15); outline: none; cursor: pointer;
}
button:hover { border-color: #396cd8; }
input { cursor: text; flex: 1; min-width: 180px; }
@media (prefers-color-scheme: dark) {
  :root { color: #f6f6f6; background-color: #2f2f2f; }
  input, button { color: #fff; background-color: #0f0f0f98; }
}
</style>
