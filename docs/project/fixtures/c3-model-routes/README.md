# F01 model-route fixtures

`three-providers.toml` resolves OpenRouter → Vercel → Open Responses in declared order. Its keys are private environment references, with no inline token. Gateway profiles use the user-selected GLM models; the synthetic Open Responses model/HTTPS endpoint are placeholders. Configuration validate/explain/render are offline. C3.11 enables multi-entry execution after all declared credentials and accounting bounds pass preflight. Replace the synthetic Open Responses profile before live use.

`single-openrouter.toml` selects only the first entry and can execute against that real provider when its declared credential is supplied. Ordinary tests override transport with a literal-loopback synthetic fixture and never make paid calls. `tests/model-routes.test.ts` reads this configuration shape, verifies render/reload, checks credential rejection before dispatch and compares actual CLI/ACP file reads to legacy OpenRouter requests. `crates/pablo-core/tests/model_routes.rs` covers nested routes, missing/duplicate/cyclic references, requirements, every-entry authority, credential scope, root/profile token narrowing and immutable child subsequences.

Run after rebuilding the debug executable:

```sh
cargo test --locked -p pablo-core --test model_routes
cargo build --locked -p pablo --bin pablo --example measure
node --test tests/model-routes.test.ts tests/model-fallback.test.ts
```

F02 adds actual HTTP 503 fallback, sticky selection, completed file history, private-continuation rejection and fresh ACP sessions. F03 complete failure/attempt-ledger proof remains C3.12.
