<script setup>
import DefaultTheme from "vitepress/theme";
import HeroBenchmark from "./HeroBenchmark.vue";
import TerminalHero from "./TerminalHero.vue";

const { Layout } = DefaultTheme;
</script>

<template>
  <Layout>
    <template #home-hero-image>
      <TerminalHero />
    </template>
    <template #home-hero-after>
      <HeroBenchmark />
    </template>
  </Layout>
</template>
