# Proposed C2: finish the single-agent foundation

This proposal was adopted at C2.0 as [cycle C2](../cycles/002-single-agent-completion.md). The candidate table below preserves the planning input; the adopted plan defines acceptance and [state](../state.json) alone records progress. C1 proves the shell/provider/ACP/telemetry path; it does not finish alpha.1 or the 0.1 release contract.

The next bounded step should complete the single-agent surface before the broader extensibility slice. This follows [section 30](../../context.md#30-suggested-implementation-sequence) and the [alpha.1 backlog entry](../backlog.md): applications need predictable file operations, machine-readable task results and explicit policy/accounting before adding more sources of work.

| Candidate checkpoint | Proposed result and acceptance |
| --- | --- |
| C2.0 — Adopt contracts and scope | Review remaining alpha.1 gaps against section 29.1; select exact filesystem, policy and accounting contracts and freeze fixture expectations. Initialize the new cycle state while retaining C1 evidence/history. |
| C2.1 — Bounded filesystem reads | Add explicit read/list/search tools through the existing registry. Prove bounded output, workspace policy, symlink behavior, cancellation and ACP/native/OTel correlation. |
| C2.2 — Safe file mutations | Add write/edit with conflict detection and documented atomicity. Prove predictable failures, preservation of unrelated content and the same lifecycle across CLI and ACP. |
| C2.3 — Single-task machine output | Add the narrow CLI JSON/task surface and structured result contract for one run. Verify stdout framing, errors, cancellation and parity with the existing ACP outcome. |
| C2.4 — Policy and accounting | Complete the selected static policy and explicit accounting limits, preserving D015's unlimited default call counts. Unknown provider usage remains unknown. Fix enforcement gaps without introducing automatic retries/fallback or unrequested confirmation UI. |
| C2.5 — Acceptance and compatibility | Exercise the completed single-agent path on macOS arm64 and Linux, add native Linux x86_64 evidence, review project-owned ACP namespace stability, and compare release measurements against C1 with matching methods. |

Keep the second gateway, Open Responses, MCP, Skills, temporary children, A2A, TUI, durable state and publishing deferred until their own cycle is selected. This proposal does not ratify the design brief's provisional performance ceilings or introduce CI regression failures.

Use the [C1.7 performance results](../evidence/c1.7.md) to prioritize investigations only when they affect this slice. Trace serialization, first-text delivery and reusable ACP setup are now optimized; retain their behavior and benchmark realistic tasks before changing ownership or format. Differences in process startup, setup, shell execution and transport need separate attribution before treating an ACP-to-core ratio as pure protocol cost.
