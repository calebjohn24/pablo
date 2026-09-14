<script setup>
import desktopChart from "../../.content/assets/resource-usage.svg";
import mobileChart from "../../.content/assets/resource-usage-mobile.svg";
</script>

<template>
  <section class="hero-benchmark" aria-labelledby="hero-benchmark-title">
    <div class="hero-benchmark-copy">
      <span>PERFORMANCE / RUNTIME FOOTPRINT</span>
      <h2 id="hero-benchmark-title">Local runtime overhead, measured</h2>
      <p>
        On six synthetic non-coding knowledge-work tasks, Pablo had the lowest
        measured CPU utilization and peak process-tree memory of Pablo, Claude Code,
        Codex and Pi.
      </p>
    </div>
    <figure class="benchmark-figure benchmark-figure-prominent">
      <picture>
        <source media="(max-width: 720px)" :srcset="mobileChart" />
        <img
          :src="desktopChart"
          alt="Bar charts comparing Pablo, Claude Code, Codex and Pi on peak process-tree memory and median CPU utilization. Pablo has the lowest measured value in both charts."
          loading="eager"
        />
      </picture>
      <figcaption>
        Exploratory single-seed pilot on macOS arm64. Models, provider routes,
        tools, reasoning settings and collection times differ.
      </figcaption>
    </figure>
    <a class="benchmark-link" href="/benchmarks">
      Read the method, exact values and limitations →
    </a>
  </section>
</template>
