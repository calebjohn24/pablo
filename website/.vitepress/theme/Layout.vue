<script setup>
import DefaultTheme from "vitepress/theme";
import FootprintHero from "./FootprintHero.vue";
import InteropHero from "./InteropHero.vue";
import RuntimeHero from "./RuntimeHero.vue";

const { Layout } = DefaultTheme;
</script>

<template>
  <Layout>
    <template #home-hero-image>
      <FootprintHero />
    </template>
    <template #home-hero-after>
      <RuntimeHero />
    </template>
    <template #home-features-after>
      <InteropHero />
    </template>
  </Layout>
</template>
