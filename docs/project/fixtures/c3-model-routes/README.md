# F01 model-route fixtures

`three-providers.toml` resolves OpenRouter → Vercel → Open Responses in declared order. Its keys are private environment references, with no inline token. Gateway profiles use the user-selected GLM models; the synthetic Open Responses model/HTTPS endpoint are placeholders. Configuration validate/explain/render are offline. Multi-entry execution is owned by C3.11 and rejects before credentials/effects at C3.10.

`single-openrouter.toml` selects only the first entry and can execute against that real provider when its declared credential is supplied. Ordinary tests override transport with a literal-loopback synthetic fixture and never make paid calls. `tests/model-routes.test.ts` reads this configuration shape, verifies render/reload, rejects premature fallback execution and compares actual CLI/ACP file reads to legacy OpenRouter requests. `crates/pablo-core/tests/model_routes.rs` covers nested routes, missing/duplicate/cyclic references, requirements, every-entry authority, credential scope, root/profile token narrowing and immutable child subsequences.

Run after rebuilding the debug executable:

```sh
cargo test --locked -p pablo-core --test model_routes
cargo build --locked -p pablo --bin pablo --example measure
node --test tests/model-routes.test.ts
```

These are F01 resolution and single-entry checks. F02/F03 runtime fallback and complete attempt-ledger proof remain their own gates.
