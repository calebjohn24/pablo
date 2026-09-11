# D01 — Offline doctor and focused diagnostics

`pablo doctor [--json] [RUN CONFIGURATION OPTIONS]` reports the installed binary,
platform and pinned protocols; selected provider/models and credential presence;
static policy, shell, Skill roots, MCP/A2A configuration, child limits and native
trace/OTel configuration. It reports its elapsed time. This is a local diagnostic
operation, with no agent execution, model request, MCP process launch or telemetry
export by default. Credential values, MCP argument/environment values, instructions
and raw provider errors are excluded from reports.

`--probe provider` explicitly permits one small model request to the selected
provider. `--probe mcp` explicitly permits configured MCP startup/negotiation and
requires joined cleanup. Probes use existing configured authority and credential
resolution, never a separate credential store. Neither probe executes an agent
or calls discovered tools. Provider acceptance is checked separately from model
output quality. Named model routes are shown; a provider probe targets the first
selected route entry without silently falling back.

The report uses stable diagnostics with cause, fix, component, configuration
provenance and exit code. Codes: 0 checked successfully; 2 invalid arguments or
configuration; 3 missing/unusable credential; 4 provider authentication rejected;
5 model/profile/request unsupported; 6 MCP startup/negotiation/cleanup failed;
7 static policy denied; 8 provider transport/response failure. Multiple findings
retain their individual codes; the first failing finding determines process exit.
Unprobed reachability is explicitly unknown and never reported healthy.

Configured deployments use the same explicit bootstrap, composition, profile and
CLI override policy as run/ACP. Reports point to source/provenance records rather
than copying complete operator-authored values. Legacy run defaults remain usable
without a deployment file, including provider-specific environment/.env precedence.
Offline credential presence does not mean the remote service accepted a key.
Native platform and measured timing evidence belong to the checkpoint report.

## Probe scope and compatibility

Provider probes use the ordinary HTTP client, authentication scope, redirect/retry
rules and request encoder. They send a fixed short input with no tools and a
sixteen-token output allowance, with a ten-second deadline. A successful probe
means a 2xx response with the expected SSE content type; response bodies are not
retained or evaluated. It is not an assertion of valid generated output, final
model completion or remote cancellation. HTTP 401/403 maps to authentication/access
rejection, 400/404/422 to model-request/endpoint rejection, and other unsuccessful
responses or transport failures to code 8. Raw remote messages never become causes.

MCP probes call the configured startup API and explicitly close the returned tool
registry. Required startup failure, optional server omission and cleanup failure
are all unhealthy. Discovered tools are never called. Existing host policy rejects
a denied server before its executable starts.

Ordinary CLI setup and provider/policy failure messages include a cause/fix pointer
to doctor. Their established setup/runtime exit codes (2/1), task envelopes and
ACP framing remain unchanged; the diagnostic-specific exit codes above apply only
to doctor. `--json` emits the version-1 doctor report; default output is a readable
report with source references and cause/fix lines. Operator-authored terminal
controls are neutralized. JSON preserves strings using JSON escaping.

Doctor checks declared credential presence using the existing private resolvers.
It never hashes or prints resolved values. MCP command arguments, environment
values, exporter header values, task instructions and resource attribute values
are omitted. Configured reports retain configuration source/provenance metadata;
legacy reports identify provider-specific environment variable names and .env
precedence. Credential presence is not remote acceptance.

Configured trace parent/exclusive-target checks create no trace file and do not
claim to prove a future write against concurrent changes. Skill discovery uses a
bounded local deadline; metadata issues are reported. Legacy OTel fields describe
requested SDK configuration without constructing an exporter. The offline timing
gate is measured on the native host, not a promise against arbitrarily stalled
filesystems or scheduler starvation.

Ctrl-C during diagnosis propagates cancellation and awaits the in-flight probe
before exit. An interrupted probe returns 130 after owned cleanup; an independently
reported cleanup failure retains its diagnostic failure code. Provider cancellation
drops the owned request without claiming remote generation stopped. MCP startup
cancellation joins the owned local process before the report is emitted.
