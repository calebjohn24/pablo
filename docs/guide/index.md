---
layout: home
title: Pablo — Rust agent runtime
titleTemplate: false
description: A small, bounded Rust agent runtime for applications doing non-coding knowledge work.
hero:
  name: Pablo
  text: The agent runtime your application can own.
  tagline: One bounded lifecycle for models, tools, policy, streaming, cancellation and traces—across a CLI, terminal, ACP and your Rust host.
  actions:
    - theme: brand
      text: Start building
      link: /getting-started
    - theme: alt
      text: Read the architecture
      link: /introduction
features:
  - title: One runtime lifecycle
    details: Model calls, tools, child work, native events and OpenTelemetry settle together. Cancellation waits for owned cleanup.
  - title: Host-owned authority
    details: Your application chooses the workspace, tools, credentials, policy, limits and approval experience.
  - title: Open interfaces
    details: Run from a terminal, speak stable ACP over stdio, attach MCP tools, load Agent Skills or embed the Rust core.
---

<section class="signal-strip" aria-label="Pablo status">
  <span><i class="signal-live"></i> prerelease development</span>
  <span>Rust 1.98.1</span>
  <span>ACP · MCP · A2A · OTel</span>
  <span>macOS + Linux</span>
</section>

## A small runtime with hard edges

Pablo is the execution layer between your application and a model. It streams a task through a typed provider, exposes only the tools your host admits, and returns one native outcome with exact accounting. It does not take over your product’s state or permission model.

<div class="interface-grid">
  <a class="interface-card" href="/cli">
    <span class="card-index">01</span>
    <h3>CLI + TUI</h3>
    <p>Run one task, emit a machine envelope, or work from a compact interactive terminal.</p>
    <strong>Explore the terminal →</strong>
  </a>
  <a class="interface-card" href="/acp">
    <span class="card-index">02</span>
    <h3>ACP host</h3>
    <p>Connect an editor or application over the stable Agent Client Protocol with streamed updates.</p>
    <strong>Integrate over stdio →</strong>
  </a>
  <a class="interface-card" href="/embedding">
    <span class="card-index">03</span>
    <h3>Rust core</h3>
    <p>Inject a provider, tracer, event sink and cancellation token into the runtime directly.</p>
    <strong>Embed the runtime →</strong>
  </a>
  <a class="interface-card" href="/configuration">
    <span class="card-index">04</span>
    <h3>Deployment</h3>
    <p>Compose model routes, tools, policy, output contracts and telemetry in inspectable TOML.</p>
    <strong>Configure a host →</strong>
  </a>
</div>

## From prompt to settled outcome

<div class="flow-line" role="img" aria-label="Task lifecycle: host admission, model stream, tools, validation, settled outcome">
  <div><span>01</span><b>Admit</b><small>config + policy</small></div>
  <i>→</i>
  <div><span>02</span><b>Stream</b><small>model + events</small></div>
  <i>→</i>
  <div><span>03</span><b>Act</b><small>bounded tools</small></div>
  <i>→</i>
  <div><span>04</span><b>Settle</b><small>outcome + usage</small></div>
</div>

Every admitted task ends as `completed`, `cancelled`, `timed_out`, `policy_denied`, `limit_exceeded`, or `failed`. Failure preserves delivery certainty, so a host can distinguish a request that was never sent from one a remote service may have received.

## Built for composition

<div class="capability-list">
  <div><b>Model routing</b><span>Vercel, OpenRouter, Open Responses, ordered fallback and private reasoning continuation.</span></div>
  <div><b>Local capabilities</b><span>Filesystem reads, revision-checked mutations and shell commands under explicit policy.</span></div>
  <div><b>Open extensions</b><span>MCP tools, portable Agent Skills, supervised local children and remote A2A tasks.</span></div>
  <div><b>Output contracts</b><span>Machine envelopes, local JSON Schema validation and one bounded repair attempt.</span></div>
  <div><b>Operations</b><span>Offline config inspection, focused diagnostics, bounded JSONL traces and OTLP export.</span></div>
</div>

::: warning Prerelease status
Pablo `v0.0.1` is being prepared and must currently be built from source. Release archives, signing limits and the full four-platform acceptance matrix are still in progress. The runtime contract is versioned and may change before 0.1.
:::

<div class="final-cta">
  <span>READY TO TRACE A REAL TASK?</span>
  <h2>Build the binary. Run the offline demo. Add a provider when you’re ready.</h2>
  <a href="/getting-started">Get started in five minutes →</a>
</div>
