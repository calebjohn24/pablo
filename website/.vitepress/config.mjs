import { defineConfig } from "vitepress";
import { fileURLToPath } from "node:url";

export default defineConfig({
  srcDir: ".content",
  outDir: "./dist",
  title: "Pablo",
  description: "A lightweight agentic harness for non-coding knowledge work, built in Rust.",
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
    ["meta", { property: "og:title", content: "Pablo — A 15× smaller harness for your agents" }],
    [
      "meta",
      {
        property: "og:description",
        content: "A lightweight agentic harness for non-coding knowledge work, built in Rust.",
      },
    ],
    ["meta", { property: "og:url", content: "https://runpablo.pages.dev/" }],
    ["meta", { property: "og:image", content: "https://runpablo.pages.dev/og.png" }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { property: "og:image:alt", content: "Pablo — A 15× smaller harness for your agents." }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:title", content: "Pablo — A 15× smaller harness for your agents" }],
    [
      "meta",
      {
        name: "twitter:description",
        content: "A lightweight agentic harness for non-coding knowledge work, built in Rust.",
      },
    ],
    ["meta", { name: "twitter:image", content: "https://runpablo.pages.dev/og.png" }],
    ["meta", { name: "twitter:image:alt", content: "Pablo — A 15× smaller harness for your agents." }],
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
      { text: "Integrate", link: "/getting-started#choose-an-integration-boundary" },
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
          { text: "Choose an interface", link: "/getting-started#choose-an-integration-boundary" },
          { text: "ACP", link: "/acp" },
          { text: "Embed in Rust", link: "/embedding" },
          { text: "Subagents", link: "/subagents" },
          { text: "MCP, Skills and agents", link: "/extensibility" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Limits and outcomes", link: "/reference" },
          { text: "Resource benchmark", link: "/benchmarks" },
          { text: "Troubleshooting", link: "/troubleshooting" },
          { text: "Version and support", link: "/release-status" },
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
      message: "Hosts own sandboxing, approvals and application state.",
      copyright: "Built for applications that need a small, inspectable agent runtime.",
    },
    docFooter: { prev: "Previous", next: "Next" },
    lastUpdated: { text: "Updated" },
  },
});
