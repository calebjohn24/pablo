# G01 deployment contract corpus

These are frozen C3.1 specification artifacts for the [contract](../../contracts/c3-deployment-config.md) and [inventory](../../contracts/c3-deployment-options.md). The existing binary cannot load them. The source [schema](../../schemas/deployment-v1.schema.json) validates document shape; semantic requirements remain part of the contract. [State](../../state.json) alone records implementation progress.

Run the offline artifact audit with the already installed Python 3.11+ and `jsonschema`/`referencing`:

```sh
python3 docs/project/fixtures/c3-deployment/audit.py
npm test
node scripts/project.mjs check
git diff --check
```

The audit reads only these named synthetic files and schemas. It validates two Draft 2020-12 schemas, the complete [baseline defaults](../../schemas/deployment-defaults-v1.json), four accepted-shape documents and fourteen intentionally invalid documents. Three [canonical vectors](canonical-vectors.json) include control escaping, UTF-8 ordering, exact counters and unsorted lists. The manually authored [production result](production.resolved.json) demonstrates a fully defaulted config, source digests, effective/input fingerprints and complete leaf/list-item provenance. Checking that example's schema, hashes and origins is **not** proof that a loader produces it.

| File | Intended observation |
| --- | --- |
| [Base module](modules/base.toml) | Shared single Vercel model, external key reference, explicit workspace binding, named inspection profile and a prepend contribution |
| [Development](development.toml) | Import/base/profile order, append then prepend, explicit writes, trace template and one optional non-secret scalar environment binding |
| [Production](production.toml) | Same base, selected read-only profile, list clearing, locked inputs, immutable tool/workspace/model/credential/capture/budget ceilings and a narrowed deadline permission |
| [Credential sources](credential-sources.toml) | Ordered environment/private-file/host references; exporter has a separate consumer and reference. No referenced file is supplied or opened |
| [Case manifest](cases.json) | Eighteen concrete parser/schema cases plus thirty exact G02/G03 semantic case descriptions, with inputs, expected results and owners; semantic cases explicitly remain `not_run` |

Use fresh temporary canonical directories for `workspace`, `secrets` and the approved configuration root in future executable tests. Do not use the real repository `.env`, actual host user config or provider credentials. Nonexistent referenced secret files are intentional: offline resolution must not open them. All example keys/paths are operator configuration, not task transcripts. `google/gemini-3.8-flash` preserves the C2 default; no live model verification is part of G01.

G02-01 compares the entire production config/fingerprint against the golden result. G02-02 fixes development's allow-rule order to `read.workspace`, `read.shared`, `read.docs`; its model call count stays unlimited and tool count is 20 until an approved override. Other G02 cases specify every merge/clear/precedence layer, cycles/duplicates/conflicts, explicit path bases, limit boundaries, changed inputs, inactive unsupported fields and hostile ambient inputs. G03 cases require actual CLI/ACP/Rust equivalence, authority intersections, narrowing, inspection/render round trips, C2 compatibility, per-run snapshots and secret isolation.

Intended owning commands after implementation:

```sh
# C3.2 must add this executable core resolver suite; it does not exist at C3.1.
cargo test --locked -p pablo-core --test deployment_config
# C3.3 extends it and adds actual executable/ACP integration coverage.
cargo test --locked -p pablo --test config
node --test tests/config.test.ts
```

The owning checkpoint may deliberately record a command/location change while preserving every expected observation. Do not use the G01 Python auditor as the production resolver or as G02/G03 behavioral acceptance. In particular, metadata saying `not_run` is a fixture handoff label, not an alternate checkpoint status authority. Paid gateway checks, all future feature execution, packaging and native release gates remain separate.
