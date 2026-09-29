# Retained legal cost experiment harness

The sibling `legal-benchmark` workspace has no Git repository. This patch retains
all runner/accounting changes and focused tests. `patch-inputs.json` fingerprints
every edited input and output; new files have a null before hash.

From an unchanged sibling baseline, run `git apply legal-benchmark.patch`, then
`python3 -m unittest discover -s tests`. Apply no changes to tasks, prompt, answer
keys, scoring or retained results. The patch separates estimates from reported
costs, accepts decimal-string u64 accounting, and polls documented generation
records after the measured attempt loop. Missing costs/cache writes remain unknown.

Run `scripts/opus_caching_experiment.py` with a pinned Linux archive, archive and
binary SHA-256, source commit, and fresh output directory. It installs and verifies
the candidate directly in E2B base sandboxes; no published installer is used for
Pablo. It runs serially: four tasks off/on and two competitor smokes, then gates on
full billing/cache observability and auto cache reads. Only a passing pilot admits
300 Pablo attempts (seeds 41/42/43 with balanced setting order) and 100 fresh
competitor attempts. Each process receives a fresh workspace, no task retries.
Pinned Claude Code 2.1.263 and SDK 0.2.154 retain medium effort. All builds/installs
precede task measurement. Task latency excludes subsequent billing polling.

Runtime/provider-default reasoning, tools, output limits, prompt and scorer are
unchanged. Session affinity, Anthropic transport and scorer repair are deferred.
Unknown billing and interrupted attempts prohibit a savings claim. Raw traces and
credentials remain local ignored data; commit only sanitized aggregate evidence.
## Measured and final harness identity

The 410 pilot/main attempts used the patch at PR commit
`46483cfb22e26eb0abc1a0c4367063e291abd3e9`, with harness SHA-256
`96ed2925944de285424bbea846286a6cda58a2ef71c974cd1b89bd674268f1cd`.
Retrieve that exact experimental patch and its hashes with `git show
46483cfb22e26eb0abc1a0c4367063e291abd3e9:docs/project/fixtures/opus-cost/legal-benchmark.patch`
and the corresponding `patch-inputs.json` at the same commit.

After measurement, the current patch adds a Vercel-only billing enrichment guard
and regression test. This preserves OpenRouter reported costs in future mixed
provider runs and has no effect on the measured Vercel rows. Its harness hash is
`5a80c6a3985d340c1be14ed4d2c1ba83ea116e42dc5e98085c7c8c6fc7ededed`.
The final patch reproduces all output hashes and passes 20 offline Python tests;
the frozen measured patch passed 19. No paid attempt was repeated for this guard.
