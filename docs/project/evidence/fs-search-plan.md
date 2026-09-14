# Filesystem search planning evidence

On 2026-09-14 the user requested a plan to improve search efficiency and effectiveness. The [proposal](../proposals/004-filesystem-search.md) records five candidate stages, exact compatibility constraints, proposed request/output changes, meaningful fixtures and measurable acceptance targets.

This planning work is logged against the existing active C3.34 solely to preserve session history. It does not satisfy B01, adopt a search checkpoint, alter the release cut line or authorize packaging. Existing README, guide, website and state edits were present at session start and preserved.

## Basis

Reviewed the current search scanner, directory traversal, read/encoding behavior, tool schemas, filesystem tests, existing performance harness, dependency pins, filesystem contract and relevant durable decisions. Checked primary upstream documentation for reusable literal matching, regex semantics, glob matching and parsing explicitly supplied ignore rules. The proposal links those sources at the relevant design choices.

The central constraint is whole-file UTF-8/binary validation: streaming may reduce retained memory, but provisional matches cannot become visible before file validation. The plan preserves work-limit and I/O-error precedence, handles giant nonmatching lines, and separates reduced scope from improvements to the same search workload. New default work quotas are excluded by D038.

## Verification

- Required session context and project consistency checks passed.
- `node --test scripts/project.test.mjs` passed all 34 existing tests.
- A one-off Node link/anchor audit passed all 25 local Markdown references in the proposal and backlog.
- Project consistency and `git diff --check` passed after writing the proposal; final record consistency is checked again after appending this session.
- Reviewed the proposal and targeted records diff. No runtime source or dependency changes were made.

Search feature acceptance, Rust runtime retesting, live provider runs and performance measurements were not run for this documentation-only task. Numerical targets in the proposal are suggestions to freeze at adoption, not observed improvements or new CI ceilings. Prior-turn filesystem tests do not constitute acceptance of proposed behavior.

## Handoff

The requested plan is complete. Its backlog entry requires selecting and adopting S1 before implementing it. C3.34 remains `in_progress` with the pre-existing documentation handoff; packaging stays held, B01 remains incomplete and no next implementation checkpoint is selected. No new durable design decision is adopted merely by writing this proposal.
