# Direct HTTP gateway and terminal preview

C1.2a enables one real task per `pablo run "TASK"` invocation through the existing Rust runtime. `pablo-core::gateway::GatewayProvider` implements `Provider` using `reqwest` 0.13.4 with Rustls TLS and streamed HTTP bodies. There is no Vercel SDK. `dotenvy` 0.15.7 is used only by the executable to parse credential files privately. Dependencies and the transitive graph are pinned in Cargo manifests and the lockfile.

## Terminal behavior

The CLI streams model text to stdout and model/shell activity to stderr. It prints the command and cwd before execution, then the shell status, exit code, and stdout/stderr byte counts after cleanup. The model receives the actual structured result and can explain it in its answer. Shell output is bounded and returned at tool completion; it is not a live terminal session.

`run` enables `shell.run` and filesystem reads by default. `--no-shell` removes shell; combine it with `--no-filesystem` for an empty tool catalog. `--workspace PATH` selects an existing directory, canonicalized before execution; it defaults to the invoking directory. The existing [shell policy and cleanup contract](shell.md) applies. This is host execution with the current user's permissions; a contained cwd does not isolate file or network access. No interactive approval UI or sandbox is introduced.

Each invocation is independent. The default provider/model is Vercel `zai/glm-5.3-flash`; `--provider openrouter` selects OpenRouter `z-ai/glm-5.3-flash`; `--model` selects an explicitly supplied compatible chat-completions model. Tool and model call counts are unlimited by default. Set `--max-tool-calls N` and/or `--max-model-calls N` to cap them; zero disables those calls. The remaining defaults are one hour per task, 15 minutes per tool, and 65,536 output tokens per model call. `--timeout` and `--tool-timeout` accept 1–86400 seconds and change the run and shell deadlines respectively. A tool deadline cannot outlast its run. Larger tasks may exhaust these bounds.

Ctrl-C cancels the shared token and continues awaiting the run. Owned shell cleanup completes before reporting cancellation; exit status is 130. Completion exits 0; other outcomes/configuration errors exit 1 with a safe reason on stderr. As with other inline sinks, a blocked output pipe or filesystem can block the calling thread; ACP bounded queues and slow-consumer/disconnect acceptance remain C1.3.

## Credentials and traces

The executable selects `AI_GATEWAY_API_KEY`, then the existing `VERCEL_AI_GATEWAY` alias, from its process environment. If neither exists, it reads `.env` in the invoking directory, or `--env-file PATH`. In the file, the canonical name wins over the alias. Duplicate occurrences of either name, malformed syntax, files over 64 KiB, and invalid keys are rejected with fixed messages. It does not search ancestor directories, source shell code, mutate the file, or install its values into the process environment. `--workspace` is independent of credential-file selection.

The adapter owns a sensitive Authorization header separately from the model request. Credentials are never put into `RunSpec`, model messages, native events, or OTel attributes. Shell uses the existing cleared environment. Credential files remain ordinary host files; isolate a workspace when it must not have access to secrets elsewhere on the host.

`--trace PATH` creates a new private JSONL file using the existing `c1.2` event schema and mapping; this adapter does not change those contracts. The parent directory must exist. Content is redacted by default. `--capture-content` requires a trace path and includes model and shell content. No network telemetry exporter is enabled.

## Wire contract and bounds

Connection establishment allows 60 seconds, subject to the remaining run deadline. The adapter posts to `https://ai-gateway.vercel.sh/v1/chat/completions` using a bearer header, `stream: true`, and `stream_options.include_usage: true`. System instructions and tool definitions stay stable between model calls. Typed user/assistant/tool history is translated into chat messages; actual `ToolResult` JSON becomes the matching tool message. The internal `shell.run` name maps to provider-valid `shell_run` in both directions. Tools are requested sequentially with `parallel_tool_calls: false`.

SSE is parsed incrementally with a 4 KiB read buffer, LF/CRLF/CR line support, comments, multiline data fields, and UTF-8 JSON decoding after frame assembly. Limits are 32 MiB per frame, 128 MiB total response bytes, 1,000,000 frames (including blank/comment frames), 128 tool-call indices, 128-byte IDs, and 64-byte function names. The runtime separately applies configured call-count caps and its tool/event/context/output limits. Serialized HTTP requests are capped at 128 MiB. A bounded frame is reduced into normalized deltas before reading the next frame; there is no unbounded event queue.

Exactly one choice is supported. Function-name and argument fragments are assembled, indices and identities validated, and unsupported/malformed content fails with a closed error. A finish reason is held until `[DONE]` so a trailing usage chunk is included. `[DONE]` ends the response; HTTP EOF before it is an error. Missing usage/cache fields remain unknown; reported values are not inferred from text. The ordinary core handles length finishes and reported output-token limits.

Redirects, automatic HTTP retries, proxies, and response decompression are disabled. Non-success HTTP responses and streamed provider errors are rejected without exposing their bodies. Opening errors distinguish not-sent from uncertain delivery; response errors record response-received. The runtime cancels opening/streaming work by dropping it at cancellation or its absolute deadline, with a 60-second connection timeout as an additional bound. This preview does not implement every model's optional modalities, reasoning parameters, fallback behavior, or structured outputs.

For offline executable fixtures only, the host may set `PABLO_FIXTURE_ENDPOINT` in the process environment. It accepts literal loopback HTTP (`127.0.0.1` or `::1`) without userinfo, query, or fragment. This path bypasses credential loading and uses a synthetic key. The model and `.env` cannot set the endpoint. The same adapter/parser handles fixtures and live traffic.

## Explicit verification

Ordinary tests use local HTTP fixtures:

```sh
cargo test --locked -p pablo --test run
cargo test --locked -p pablo-core gateway::tests
```

The following command makes a small paid live request using the supplied key. It creates a temporary evidence file, asks the model to read it with the shell, asserts the actual evidence in the answer, and leaves a redacted trace and summary under ignored `.pablo/traces`. The temporary workspace is removed afterward.

```sh
cargo build --locked -p pablo
node scripts/smoke-live.mjs
```

An optional argument selects a previously built binary, for example `node scripts/smoke-live.mjs target/release/pablo`. This is not part of ordinary automated tests. Live CLI evidence does not mark ACP, Collector, Linux, or the full cycle accepted.

For the formal C1.4 live ACP model/shell/model check, run `npm run smoke:live:acp`. It uses the same gateway transport; see [live ACP acceptance](acp.md#live-acceptance) for its bounds, credential selection and evidence.

The request format follows Vercel's [REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/rest-api), [streaming](https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/streaming), and [tool calling](https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/tool-calling) documentation, inspected on 2026-09-05. The transport uses the pinned [reqwest API](https://docs.rs/reqwest/0.13.4/reqwest/).

When an explicit tool-call cap is exhausted, or the current model call is the last one allowed, the gateway sends `tool_choice: "none"` to request an answer from collected evidence. Tool definitions and the instruction prefix remain stable. The runtime still rejects extra calls if the provider ignores the hint. See [Vercel tool choice](https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/tool-calling).

## Open Responses

C3.9 adds `open_responses` to the configured Rust, CLI and ACP paths. Select a complete HTTPS URL, an endpoint-specific model and the literal capability profile; no model or endpoint is inferred. Supply the named environment credential privately:

```toml
schema_version = 1
[credentials.responses]
consumer = "provider.open_responses"
sources = [{kind = "environment", name = "EXAMPLE_RESPONSES_TOKEN"}]
[options.model]
provider = "open_responses"
id = "your-endpoint-model"
endpoint = "https://your-provider.example/v1/responses"
capability_profile = "open-responses-text-tools-v1"
credential = "responses"
[options.shell]
enabled = false
```

Save this as `responses.toml`, replace the endpoint/model, and run:

```sh
pablo config validate --config responses.toml --bind workspace=/absolute/workspace
pablo run "Read README.md and summarize it." --config responses.toml --bind workspace=/absolute/workspace
pablo acp --stdio --config responses.toml --bind workspace=/absolute/workspace
```

Validation and inspection are offline. Execution uses only this credential reference and exact destination; it does not load the implicit Vercel/OpenRouter keys. `auth_scheme` defaults to `bearer`; `raw` and selected custom application auth headers are supported as specified in [the pinned contract](project/contracts/c3-open-responses.md). Header values remain private and redirects are disabled. Rust hosts resolve and prepare the same deployment, then use `GatewayProvider::configured`; direct hosts may construct `OpenResponsesProfile` and `OpenResponsesProvider` explicitly.

The implemented profile supports named HTTP/SSE text, fragmented function calls, sequential tool rounds, full ordered input history, assistant phases, opaque reasoning/summaries and token/cache-read usage. Continuation is private even with content capture, scoped to the task and endpoint/model/revision/profile, and discarded on completion or cancellation. Raw reasoning that cannot be represented in upstream input, images/audio, hosted tools, structured output, background/WebSocket/compaction and extension semantics fail with a closed error. Cost and cache-write usage remain unknown; no hard accounting bounds are attested. Output allowance must be at least 16 tokens.

Run `npm run test:deployment` for offline gateway and Open Responses acceptance. OR02 exercises the pinned streams through the Rust host, CLI and official ACP client with actual file reads, cancellation/socket closure, exact accounting and privacy checks. No paid Open Responses endpoint is needed for this gate.
