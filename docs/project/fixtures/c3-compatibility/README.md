# C3.33 compatibility identities

`docs/protocol-compatibility.json` names the supported operations, explicit exclusions and executable conformance gates. `docs/protocol-pins.json` freezes their schema/fixture bytes, Cargo SDK release checksums, npm release integrities and installed independent Python SDK source identities. It also records immutable upstream revisions. The SDK-generated MCP schemas describe the SDK's union of versions; Pablo still negotiates exactly 2025-11-25 and checks this against independent peers.

Run `python3 scripts/verify-protocol-pins.py` from the checkout. It is offline, has no update mode and fails on missing fixture environments or changed SDK sources. It never loads credentials. Set up the MCP and A2A fixture environments using their checked-in requirements before running it. `python3 scripts/verify-protocol-pins.test.py` proves that fixture, Cargo, npm and Python source drift is rejected without changing production pins. Passing this identity audit is distinct from passing the runtime gates in the operation matrix.

`c2-*` files are untouched `git show` bytes from C2 revision `ba8b0ff62b3ccc40c8a6ae704b4450ead12040b3`. The relocated client is executed with an explicit binary path in `tests/compatibility.test.ts`. The public v1 metadata schema is identical to the frozen C2 schema. The current v2 schema and versioned custom methods are separate contracts.

`upstream/` contains unmodified reference data, with the corresponding licenses:

- Agent Skills specification, validator and validator tests from `69ef37e9424c0a7ea9dd2293b559e43ec8176379`; paths are recorded in the pin manifest. The copied validator is reference source, not a runtime dependency or a claim that its entire test suite was executed.
- A2A specification and proto from `173695755607e884aa9acf8ce4feed90e32727a1`, cross-checked against the existing A2A receipt.
- MCP client/server JSON-RPC schema fixtures from rmcp `3e636cab26c013eca5131103c03d20237f12c4df`.
- Draft 2020-12 metaschema and vocabulary files from the pinned `jsonschema-specifications==2025.9.1` distribution. Pablo implements only the output-validation contract's offline subset.

Open Responses retains its existing untouched upstream corpus, identities and executable audit in `../c3-open-responses/`. Configuration retains its independent canonical/shape corpus in `../c3-deployment/`. No live response, credential, raw task trace or build output belongs here.
