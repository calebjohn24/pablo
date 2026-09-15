<script setup>
import TerminalHero from "./TerminalHero.vue";
</script>

<template>
  <section class="runtime-hero" aria-labelledby="runtime-hero-title">
    <div class="runtime-hero-copy">
      <span>// PABLO</span>
      <h2 id="runtime-hero-title">The agent runtime your application can own.</h2>
      <p>
        Embed one bounded lifecycle for models, tools, policy, streaming,
        cancellation and traces through ACP or directly in your Rust host.
      </p>
      <div class="runtime-hero-actions">
        <a class="runtime-button brand" href="/getting-started">Install and integrate</a>
        <a class="runtime-button alt" href="/introduction">Read the architecture</a>
      </div>
    </div>
    <TerminalHero />
  </section>
</template>
