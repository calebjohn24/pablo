# T01–T02 — Basic streaming terminal client

C3.29 adds the reference composer and native-event view. C3.30 owns the broader
PTY stress, resize, tool/child cancellation and failure/restoration matrix.
The terminal is presentation: it has no private provider loop, credential store,
approval flow, conversation persistence or authority of its own.

## Selection and task lifecycle

| Invocation | Behavior |
| --- | --- |
| `pablo` with terminal stdin/stdout and non-dumb TERM | Open the composer |
| `pablo` with redirected input/output or unavailable TERM | Existing help text |
| `pablo tui [TASK] [RUN OPTIONS...]` | Explicit composer; optional initial task |
| `pablo run TASK`, `pablo TASK`, `demo` | Existing streaming CLI behavior, including on terminals |
| `run --json`, `acp --stdio`, config/Skill inspection | Existing machine/protocol framing |

Explicit TUI requires stdin and stdout on the same supported Unix terminal and
rejects redirected invocation before changing modes. `--json` cannot combine with
TUI; a configured JSON CLI-output mode also rejects task admission. Use ordinary
`run` for that output format. Help/version/ACP do not auto-enter the terminal.

Each submitted task clones invocation options with only the new input and calls
the same `run_options` function as the CLI. It resolves a fresh run, provider,
tools, supervisor, limits and trace identity through existing code. Display data
never enters a subsequent task's context. Credentials remain exclusively in the
existing executable resolver and are not requested while the composer is idle.
Trace files retain exclusive creation: use configured per-session trace paths for
multiple tasks; reusing a constant legacy trace filename fails safely.

## Controls and bounded display

Enter submits the current nonempty task. Left/right, Home/End, Backspace and Delete
edit UTF-8 input at character boundaries. Bracketed paste retains newlines without
submitting them; Enter after paste submits. The insertion point uses a visible
marker rather than terminal cursor coordinates derived from untrusted text.

Ctrl-C during a run cancels its real cancellation token and awaits owned cleanup;
it leaves the composer available for another independent task. Ctrl-C while idle
exits with 130. Ctrl-D while idle with empty input exits; during a run it cancels,
joins and exits. An external SIGTERM joins the active run and exits with 143.
Input EOF exits after joining any active task. The last task's exit status survives
normal exit (0 success, 1 failed/limited/denied/timed out, 130 cancelled); pre-admission
setup failures use 2. A display failure uses 1 and input failure uses 2.

The view shows streamed narration with explicit terminal outcome, provider/model,
current tool and owned-agent activity, activated Skill names, observed local
usage, static-policy status and the native trace ID. While running, usage reflects
observed native operations; the root terminal accounting replaces it with the
actual aggregate. Unknown usage/cost stays unknown. Remote reported spending never
becomes a local charge. Queued handles and settled stop/wait receipts update the
same bounded activity display, including work that never started.

| Presentation resource | Bound |
| --- | --- |
| Composer | 16 KiB UTF-8 |
| Retained narration | 64 KiB, dropping the oldest prefix at character boundaries |
| Metadata labels | 256 bytes each |
| Activity entries / Skill labels | 17 / 16 |
| Render viewport | At most 240 columns and 80 rows |
| Refresh / input polling | At most 20 frames/s / nonblocking 5 ms input polling |
| Frame output / restoration attempt | 500 ms / 250 ms |

Only trusted renderer literals emit terminal control sequences. Untrusted C0/C1
controls and directional override/isolate characters become visible replacement
characters; transcript newlines remain line breaks. Labels flatten newlines.
Remote tool arguments, Skill bodies and credential/policy data are not copied into
the display. Non-ASCII characters are conservatively budgeted as two columns;
small terminals show a resize message. These display bounds do not replace runtime
input/output/work quotas and do not truncate the provider's actual result.

## Ownership and restoration

The runtime writes native events into bounded in-memory presentation state without
waiting for screen I/O. A separate async renderer and nonblocking input poll run
beside the same task future. Slow or failed rendering cancels that future and
joins it; the UI never drops a running tool/child future as its cancellation path.
No detached input/render thread is introduced.

The terminal guard captures attributes before raw mode and restores modes,
bracketed-paste setting, cursor and alternate screen on normal/error exit, with a
bounded best-effort write if the device stops accepting output. The macOS fixture
keeps its terminal owner alive and drains output during process exit. It checks
attributes after canonical handoff, allowing the kernel-owned PENDIN retype state
to clear; no configured flags are masked out. SIGKILL cannot run cleanup. Full
platform-specific restoration/slow-output claims require C3.30 and native gates.
