<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const greetMsg = ref("");
const url = ref("");

async function setTrayUrl() {
  await invoke("set_tray_url", { url: url.value });
  greetMsg.value = `Tray will open: ${url.value}`;
}
</script>

<template>
  <main class="container">
    <h1>minibro</h1>
    <form class="row" @submit.prevent="setTrayUrl">
      <input id="url-input" v-model="url" placeholder="Enter a URL..." />
      <button type="submit">Save</button>
    </form>
    <p>{{ greetMsg }}</p>
  </main>
</template>

<style scoped>
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
  align-items: center;
  text-align: center;
}

.row {
  display: flex;
  justify-content: center;
  gap: 8px;
}

input, button {
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
  outline: none;
}

button { cursor: pointer; }
button:hover { border-color: #396cd8; }
button:active { border-color: #396cd8; background-color: #e8e8e8; }

#url-input { width: 300px; }

@media (prefers-color-scheme: dark) {
  :root { color: #f6f6f6; background-color: #2f2f2f; }
  input, button { color: #ffffff; background-color: #0f0f0f98; }
  button:active { background-color: #0f0f0f69; }
}
</style>
