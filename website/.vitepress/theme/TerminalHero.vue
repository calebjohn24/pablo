<script setup>
import { onMounted, onUnmounted, ref } from "vue";

const commands = [
  'pablo run "Map this repository." --no-shell',
  'pablo run "Return the findings." --json --no-shell',
  "pablo acp --stdio",
];
const active = ref(0);
let timer;

onMounted(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  timer = setInterval(() => {
    active.value = (active.value + 1) % commands.length;
  }, 4200);
});
onUnmounted(() => timer && clearInterval(timer));
</script>

<template>
  <div class="terminal-shell" aria-label="Pablo terminal example">
    <div class="terminal-bar">
      <span class="terminal-dot red" />
      <span class="terminal-dot amber" />
      <span class="terminal-dot green" />
      <span class="terminal-title">pablo · ~/project</span>
    </div>
    <div class="terminal-body">
      <div class="terminal-command">
        <span class="terminal-prompt">$</span>
        <Transition name="command" mode="out-in">
          <code :key="active">{{ commands[active] }}</code>
        </Transition>
      </div>
      <div class="terminal-event muted">
        <span>01</span><b>run.started</b><em>trace 7a93…</em>
      </div>
      <div class="terminal-event">
        <span>02</span><b>tool.started</b><em>fs.list</em>
      </div>
      <div class="terminal-event">
        <span>03</span><b>tool.finished</b><em>42 entries · 3.1ms</em>
      </div>
      <div class="terminal-event accent">
        <span>04</span><b>model.delta</b><em>I found two runtime crates…</em>
      </div>
      <div class="terminal-result">
        <span class="status-light" /> completed
        <small>2 model calls · 1 tool call</small>
      </div>
    </div>
  </div>
</template>
