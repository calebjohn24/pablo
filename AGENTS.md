# Working on pablo

## Start each session

1. Inspect `git status --short` and preserve existing user work.
2. Run `node scripts/project.mjs context` and `node scripts/project.mjs check`.
3. Read the selected checkpoint in the cycle plan. Read relevant sections of `docs/context.md` only as needed; section 29.1 defines the full 0.1 release contract and section 37.1 defines the first spike.
4. Resume the active checkpoint or select `next_checkpoint` from `docs/project/state.json`. Mark it `in_progress`, set `active_checkpoint`, and clear `next_checkpoint` before implementation.

Use one checkpoint per implementation session, as requested by the user. Finish its verification and handoff, then stop with the next checkpoint ready. Explicit user instructions can change this cadence. A checkpoint may span multiple sessions if interrupted; partial work remains `in_progress`.

## Records and authority

- `docs/context.md` holds the product design. Preserve its rationale and release cut line.
- The cycle specification selected by `docs/project/state.json` holds checkpoint specifications and acceptance criteria. Plans live under `docs/project/cycles/` and do not maintain status checklists; retain prior plans and checkpoint records when adopting a new cycle.
- `docs/project/state.json` is the only authoritative current task state. Readiness is derived from completed dependencies.
- `docs/project/brain.md` holds durable decisions and orientation, within 200 lines. Use stable `D001`-style decision IDs and explain why each decision exists. Link detailed design instead of copying the brief.
- `docs/project/log.jsonl` is append-only history. Correct a historical error in a new entry; do not rewrite old entries.
- `docs/project/backlog.md` holds deferred work and promotion conditions. Do not start a future slice just because it appears in the brief.
- Runtime traces, credentials, and build output are local ignored data. They do not belong in the project brain or work log.

Current code and observed checks determine implementation truth. If records disagree with them, investigate and correct the records before claiming completion. A changed context hash requires a deliberate review of the brain and cycle plan; do not update the hash merely to make a check pass.

## State format (version 1)

Top-level fields identify the cycle and its specification, document paths, the reviewed source context (`path`, SHA-256, review timestamp), update timestamp, active/next checkpoint, next action, latest log entry, and checkpoint records. All timestamps are UTC ISO 8601.

Each checkpoint has `id`, `title`, `spec` (repository-relative Markdown path and heading anchor), `depends_on`, `status`, `blockers`, and `verification` (log-entry IDs). Allowed statuses are `pending`, `in_progress`, `blocked`, and `done`.

At most one checkpoint is `in_progress`, and it must match `active_checkpoint`. A blocked checkpoint has a concrete blocker and unblock condition. A done checkpoint has completed dependencies and references log entries for that checkpoint with passing checks and checked-in evidence paths. With no active checkpoint, `next_checkpoint` selects a pending checkpoint whose dependencies are done; leave it null if none is ready. Never mark a skipped or unrun acceptance gate as passed.

## Log format (version 1)

Each nonempty line is a JSON object with:

- `id`: unique, increasing `LOG-0001`-style identifier.
- `timestamp`: UTC ISO 8601; chronological order.
- `checkpoint`: an existing checkpoint ID.
- `summary`, `changes`, `decisions`, `problems`, and `handoff`: concise human-readable context; decisions reference IDs in the brain.
- `checks`: objects containing `command`, `result` (`passed`, `failed`, or `not_run`), and `evidence` (a repository-relative file path or null).

Append actual results after running checks. Keep detailed summaries in small evidence documents under `docs/project/evidence/`; exclude secrets, raw task content, and entire terminal transcripts. The helper validates references and consistency, not whether a command was really run or acceptance was substantively met.

## End each session

1. Run checks appropriate to the checkpoint. Tests must demonstrate useful behavior; do not add tests merely for documentation wording.
2. Add an evidence summary and append a log entry with changes, results, decisions, problems, and a precise handoff.
3. Update state, including `latest_log_entry`, `updated_at`, status, verification references, and next action. Promote durable findings into the brain.
4. Run project consistency checks and inspect the final diff. Keep implementation and its records in the same reviewable change.
5. Report the completed checkpoint, verification, and next checkpoint. If incomplete, record exactly how to resume.

Use existing Rust and Node installations; `.nvmrc` pins Node. The root `.env` contains the user-provided Vercel credential. Do not print it, copy it into records, commit it, or expose it to shell tools. The helper does not load environment files. Live provider checks are explicit. The user-authorized C1.2a preview permits a small live CLI smoke; formal live ACP acceptance remains C1.4 or later.
