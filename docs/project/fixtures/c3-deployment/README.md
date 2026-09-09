# Deployment configuration corpus

These C3.1 fixtures specify the [contract](../../contracts/c3-deployment-config.md) and [inventory](../../contracts/c3-deployment-options.md). C3.2 runs them through `pablo_core::deployment`; C3.3 runs executable inspection and configured admission through the existing runtime. The source [schema](../../schemas/deployment-v1.schema.json) validates document shape; the Rust resolver also enforces semantic requirements. [State](../../state.json) alone records checkpoint progress.

Run the offline artifact audit with the already installed Python 3.11+ and `jsonschema`/`referencing`:

```sh
python3 docs/project/fixtures/c3-deployment/audit.py
npm test
node scripts/project.mjs check
git diff --check
```

The audit reads only these named synthetic files and schemas. It validates two Draft 2020-12 schemas, the complete [baseline defaults](../../schemas/deployment-defaults-v1.json), four accepted-shape documents and fourteen intentionally invalid documents. Three [canonical vectors](canonical-vectors.json) include control escaping, UTF-8 ordering, exact counters and unsorted lists. The [production result](production.resolved.json) contains defaulted config, source digests, both fingerprints and complete provenance. C3.2 refines its originally hand-authored origin labels under D035, preserving its config, source manifest and fingerprints; the Rust suite compares the entire result exactly. D038 subsequently changes the four filesystem defaults to `"unlimited"` at the user’s request; C3.3 refreshes the effective fingerprint, defaults digest and derived input fingerprint while preserving source-file hashes and provenance. The Python audit alone does not prove resolver behavior.

| File | Intended observation |
| --- | --- |
| [Base module](modules/base.toml) | Shared single Vercel model, external key reference, explicit workspace binding, named inspection profile and a prepend contribution |
| [Development](development.toml) | Import/base/profile order, append then prepend, explicit writes, trace template and one optional non-secret scalar environment binding |
| [Production](production.toml) | Same base, selected read-only profile, list clearing, locked inputs, immutable tool/workspace/model/credential/capture/budget ceilings and a narrowed deadline permission |
| [Credential sources](credential-sources.toml) | Ordered environment/private-file/host references; exporter has a separate consumer and reference. No referenced file is supplied or opened |
| [Case manifest](cases.json) | Eighteen concrete parser/schema cases and thirty G02/G03 descriptions; 21 G02 cases link passing Rust tests/evidence, and nine G03 interface/runtime cases link C3.3 evidence |

Executable tests use fresh temporary canonical directories for `workspace`, `secrets` and the approved configuration root. Do not use the real repository `.env`, actual host user config or provider credentials. Nonexistent referenced secret files are intentional: offline resolution must not open them. All example keys/paths are operator configuration, not task transcripts. `google/gemini-3.8-flash` preserves the C2 default; no live model verification is part of G01/G02.

G02-01 compares the entire production config/fingerprint against the golden result. G02-02 fixes development's allow-rule order to `read.workspace`, `read.shared`, `read.docs`; its model call count stays unlimited and tool count is 20 until an approved override. Other G02 cases specify every merge/clear/precedence layer, cycles/duplicates/conflicts, explicit path bases, limit boundaries, changed inputs, inactive unsupported fields and hostile ambient inputs. G03 cases require actual CLI/ACP/Rust equivalence, authority intersections, narrowing, inspection/render round trips, C2 compatibility, per-run snapshots and secret isolation.

Owning C3.2 commands:

```sh
cargo test --locked -p pablo-core --test deployment_config
cargo test --locked -p pablo-core --lib deployment::
```

G02-13 exercises each listed limit at the boundary and one excess. The defensive 128-edge cap and 64-environment-name cap use structural seams: accepted graphs reach 64 files first, and the current schema has fewer than 64 unique scalar targets. Node/depth/array, provenance and encoding bounds also have direct seam tests. The file/profile, credential, binding, authority and rule tests execute full resolution. See [C3.2 evidence](../../evidence/c3.2.md).

C3.3 interface/telemetry commands:

```sh
npm run test:deployment
```

The owning checkpoint may deliberately record a command/location change while preserving every expected observation. Do not use the Python auditor as the production resolver or as G02/G03 behavioral acceptance. Case coverage metadata is an evidence index, not an alternate checkpoint status authority. Paid gateway checks, future feature execution, packaging and native release gates remain separate.
