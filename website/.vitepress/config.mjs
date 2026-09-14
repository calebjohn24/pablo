import { defineConfig } from "vitepress";
import { fileURLToPath } from "node:url";

export default defineConfig({
  srcDir: ".content",
  outDir: "./dist",
  title: "Pablo",
  description: "A small Rust agent runtime for applications doing non-coding knowledge work.",
  lang: "en-US",
  cleanUrls: true,
  lastUpdated: true,
  appearance: "dark",
  sitemap: { hostname: "https://runpablo.pages.dev" },
  vite: {
    publicDir: fileURLToPath(new URL("../.public", import.meta.url)),
    esbuild: { target: "es2022" },
    build: { target: "es2022" },
  },
  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" }],
    ["link", { rel: "apple-touch-icon", href: "/apple-touch-icon.png" }],
    ["meta", { name: "theme-color", content: "#0b0d10" }],
    ["meta", { name: "color-scheme", content: "dark light" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:title", content: "Pablo — Rust agent runtime" }],
    [
      "meta",
      {
        property: "og:description",
        content: "A small, bounded agent runtime for non-coding knowledge work.",
      },
    ],
    ["meta", { property: "og:url", content: "https://runpablo.pages.dev/" }],
    ["meta", { property: "og:image", content: "https://runpablo.pages.dev/og.png" }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { property: "og:image:alt", content: "Pablo — the agent runtime your application can own." }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:image", content: "https://runpablo.pages.dev/og.png" }],
  ],
  markdown: {
    lineNumbers: true,
    theme: { dark: "github-dark", light: "github-light" },
  },
  themeConfig: {
    siteTitle: "PABLO",
    nav: [
      { text: "Docs", link: "/getting-started" },
      { text: "Configuration", link: "/configuration" },
      { text: "Integrate", link: "/acp" },
      { text: "v0.0.1", link: "/release-status" },
    ],
    sidebar: [
      {
        text: "Start",
        items: [
          { text: "Introduction", link: "/introduction" },
          { text: "Getting started", link: "/getting-started" },
          { text: "Installation", link: "/installation" },
          { text: "CLI and TUI", link: "/cli" },
        ],
      },
      {
        text: "Operate",
        items: [
          { text: "Deployments", link: "/configuration" },
          { text: "Models and providers", link: "/providers" },
          { text: "Tools and policy", link: "/tools-and-policy" },
          { text: "Structured output", link: "/structured-output" },
          { text: "Observability", link: "/observability" },
        ],
      },
      {
        text: "Integrate",
        items: [
          { text: "ACP", link: "/acp" },
          { text: "Embed in Rust", link: "/embedding" },
          { text: "MCP, Skills and agents", link: "/extensibility" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Limits and outcomes", link: "/reference" },
          { text: "Resource benchmark", link: "/benchmarks" },
          { text: "Troubleshooting", link: "/troubleshooting" },
          { text: "Release status", link: "/release-status" },
        ],
      },
    ],
    outline: { level: [2, 3], label: "On this page" },
    search: { provider: "local" },
    editLink: {
      pattern: "https://github.com/calebjohn24/pablo/edit/main/docs/guide/:path",
      text: "Edit this page on GitHub",
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/calebjohn24/pablo" },
    ],
    footer: {
      message: "Pablo is prerelease software. Hosts own sandboxing, approvals and application state.",
      copyright: "Built for applications that need a small, inspectable agent runtime.",
    },
    docFooter: { prev: "Previous", next: "Next" },
    lastUpdated: { text: "Updated" },
  },
});
