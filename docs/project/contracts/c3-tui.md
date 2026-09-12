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
edit UTF-8 input at character boundaries. Page Up/Down, up/down arrows and the mouse wheel scroll retained history, including during a run. Ctrl-Home jumps to the oldest retained text; Ctrl-End returns to the live tail. Bracketed paste retains newlines without
submitting them; Enter after paste submits. The insertion point uses a visible
marker rather than terminal cursor coordinates derived from untrusted text.

Ctrl-C during a run cancels its real cancellation token and awaits owned cleanup;
it leaves the composer available for another independent task. Ctrl-C while idle
exits with 130. Ctrl-D while idle with empty input exits; during a run it cancels,
joins and exits. An external SIGTERM joins the active run and exits with 143.
Input EOF exits after joining any active task. The last task's exit status survives
normal exit (0 success, 1 failed/limited/denied/timed out, 130 cancelled); pre-admission
setup failures use 2. A display failure uses 1 and input failure uses 2.

The view retains earlier prompts and answers in the current terminal session, including persistent tool start/completion entries with names, call IDs and status. Per-task metadata resets on submission while visible history remains. Basic Markdown renders headings, emphasis, lists, quotes, links and inline/fenced code. Wrapping counts visible columns rather than ANSI formatting bytes.

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
| Refresh / input polling | At most 20 changed frames/s; no idle writes / nonblocking 5 ms input polling |
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
Native restoration/slow-output evidence is recorded per platform; the T02 procedure below defines the required checks for remaining native gates.

## Reproducible T02 native checks

`node --test tests/tui.test.ts` runs redirected selection/framing checks, the T01
smoke and eleven T02 lifecycle cases against `target/debug/pablo`. Build first
with `cargo build --locked -p pablo`. Python is the existing pinned A2A fixture
interpreter at `.pablo/a2a-fixture-venv/bin/python`; absent Python or Windows skips
these PTY cases and cannot count as acceptance.

For release checks, build with `cargo build --release --locked -p pablo --bin
pablo --example measure`, then run the fixture with that binary and each case:

```
.pablo/a2a-fixture-venv/bin/python tests/fixtures/tui/lifecycle.py target/release/pablo success
```

Cases: `success`, `model_cancel`, `model_eof`, `input_eof`, `tool_cancel`,
`provider_error`, `resize`, `slow_output`, `sigterm`, `local_child`, `remote_child`.
Each owns a real PTY and isolated synthetic provider, inspects exactly one native
root terminal result, waits for Pablo to exit, and checks the original terminal
attributes plus cursor/paste/alternate-screen restoration. Child cases also check
one child terminal result before root settlement; remote cancellation must reach
the pinned SDK peer exactly once. Shell cancellation checks its recorded owned
process no longer exists. Fixtures use no live credentials.

`model_eof` exercises the Ctrl-D control; `input_eof` forces an actual zero-byte
terminal read using VMIN=VTIME=0 and checks restoration to the attributes saved
before Pablo started. `slow_output` withholds master reads until the native trace
records cancellation caused by the frame-write deadline, then resumes draining
through restoration. This does not promise control sequences can reach a
permanently disconnected terminal. `provider_error` injects malformed SSE JSON.

On macOS, the owner remains alive while the fixture performs a canonical read to
clear kernel-managed PENDIN state before comparing every termios attribute. No
flags are masked. This is native macOS arm64 proof; each remaining platform gate
must run its native PTY cases and record OS-specific behavior. Full grapheme-width
layout and restoration after SIGKILL or terminal destruction remain outside the
contract.

## C3.33a rendering fix

The terminal is cleared once on entry. Subsequent paints update only changed rows, bracketed by synchronized-output markers where supported. Row erasure removes stale suffixes without exposing an empty whole screen. Unchanged display revisions skip Markdown/layout work and emit no terminal bytes. Mouse, synchronized-output and style modes are reset during restoration and the drop fallback.

Display history remains capped at 64 KiB, with an explicit oldest-text-discarded marker. Scrolled history stays anchored as new output arrives; submission returns to the current tail. Each task still receives independent runtime context. The Markdown renderer emits only its own fixed SGR codes after native text controls are neutralized.

`tests/fixtures/tui/display.py` inspects actual PTY screen cells and styles, verifies persistent completed tools and earlier tasks, scrolling during a model run, mouse navigation, exactly one full clear, zero idle bytes, fresh runtime context and restoration. The slow-consumer fixture continuously changes visible content so its pressure comes from real updates.
