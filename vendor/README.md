# OpenTelemetry explicit-configuration patches

These are the published Apache-2.0 sources for `opentelemetry_sdk` 0.32.1 and
`opentelemetry-otlp` 0.32.0, selected through the workspace's crates.io patches.
Versions and transitive dependency pins are unchanged. `upstream-sha256.json`
records every copied upstream file before editing; Cargo cache metadata,
package lockfiles and unnormalized manifests are excluded. Licenses were restored
from the exact upstream commits recorded in the published crate metadata:

- SDK: `284a37d93b3856e1975c2807ba3af1421ebd9b52`
- OTLP: `ec289cb3c6f8260951699c51df968560943c1451`

Three upstream source files have behavioral patches:

- SDK `src/trace/provider.rs`: add `builder_without_environment`, bypassing
  implicit resource and span-limit environment detectors.
- SDK `src/trace/span_processor.rs`: add explicit batch configuration construction
  without reading process variables before applying configured values.
- OTLP `src/exporter/http/mod.rs`: add opt-in `without_environment` so configured
  endpoint, protocol, timeout, compression and headers cannot be overridden or
  augmented by ambient variables. Update one test builder for the new field.

Two SDK metrics files (`meter.rs` and `periodic_reader_with_async_runtime.rs`) also have four upstream trailing-whitespace lines normalized for the repository diff check; they have no behavior changes.

Existing builder behavior remains the default for legacy invocations. Configured
Pablo tasks select the explicit builders. Passing headers through the original
builder alone is insufficient: the upstream implementation merges ambient
headers after the supplied header map. These patches avoid process-global
variable mutation and allow concurrent embedded deployments with separate inputs.

Workspace tests exercise the consumed APIs; they do not claim the entire upstream
feature matrix. Full configured exporter and resource-rotation acceptance remains
part of C3.3 before PR #4 can merge. Remove these patches when a pinned upstream
release provides equivalent explicit construction and passes those fixtures.
