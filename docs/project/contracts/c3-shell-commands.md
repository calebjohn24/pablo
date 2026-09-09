# C3.4 literal shell command policy

This freezes Q01 before implementation. Deployment schema version remains 1;
contract revision advances to `c3.4`. Ordinary shell behavior and native/task wire
revisions remain unchanged when no command rules are supplied. The option
inventory also assigns shell environment defaults/restrictions and cwd rules to
this checkpoint.

## Typed settings

`options.shell.commands` is optional. When supplied, it has required `default`
(`allow` or `deny`) and `allow`/`deny` rule lists, defaulting to empty. A rule has
`id`, absolute `executable`, `args` (arguments after the program), and `match`
(`exact` or `prefix`). Prefix means a prefix of whole arguments, never a string
substring. Lists replace under ordinary composition; empty arrays clear. These new dimensions do not accept `unset`. Omitted fields inherit; a supplied command object continues to require literal parsing even with no rules. IDs use the existing
64-byte policy ID syntax, remain globally unique across ordinary/authority tool,
launcher, filesystem, cwd and command rules, and count toward the existing
1,024-rule aggregate cap. Each dimension permits at most 128 rules; command
argument lists permit at most 256 strings, each at most 8,192 bytes and at most
65,536 bytes combined. Empty argument strings are meaningful.

`options.shell.environment` has `values` (non-secret `PABLO_TASK_` string defaults,
empty by default) and optional `allowed_names` (an exact list, with absence
unconstrained and an empty list admitting no additions). Model-supplied values
override ordinary defaults. The effective map has at most 32 names, each name
uses the established 128-byte syntax, and each value is at most 8,192 bytes
without NUL. Fixed PATH/LANG, cleared inheritance and forbidden credential names
remain. `options.shell.cwd_roots` is an optional existing relative-root rule
object, with explicit default and allow/deny IDs. It checks the canonical cwd
relative to the canonical admitted workspace and still cannot escape it.

Each authority layer can supply `shell.commands`, `shell.cwd_roots` and
`shell.environment.allowed_names`. Authority environment values are not defaults
and are rejected. A missing dimension adds no constraint. Every supplied layer
must allow the request; explicit deny wins, then a nonempty allowlist requires a
match, otherwise the explicit default decides. Ordinary/profile replacement
cannot erase an authority layer. Root/child hosts pass the same independent
restrictions through the configured registry; no child runtime is introduced.

## Literal parser and dispatch

Any supplied command-rule layer enables the literal parser, including deny-only
rules. The command is at most 65,536 UTF-8 bytes and 257 words including the
program. Spaces/tabs delimit words. Single quotes preserve all non-control bytes;
double quotes preserve ordinary bytes and support escaping quote, backslash,
dollar and backtick. Outside quotes, backslash escapes the next non-control
character. Empty quoted arguments and concatenated quoted/unquoted segments are
preserved. Unclosed quotes, dangling escapes, controls/newlines, unquoted shell
operators (`|&;<>()`), substitutions/expansion (`$` and backtick), globs, braces,
tilde and comments reject before launch. Metacharacters quoted/escaped into
literal arguments are data. Assignment prefixes cannot be the program name.

A bare executable is searched only in fixed `/usr/bin:/bin`. Explicit executable
paths must be absolute; relative paths with slashes reject. Resolve symlinks to a
canonical executable regular file. Rule aliases resolve to the same canonical
path; Unix device/inode identity additionally recognizes hard links. Evaluate
that resolved program and the parsed arguments. Dispatch `/bin/sh -c 'exec "$@"'
with a fixed argv[0] and the resolved program/arguments as separate positional
parameters. This retains the existing launcher policy while avoiding a second
interpretation of task command text. C2 unrestricted execution continues using
`/bin/sh -c <command>` when all command-rule dimensions are absent.

Unsupported syntax or resolution failures produce stable metadata-only configured
policy denials. Successful decisions preserve each matching rule ID; no policy
error embeds command text, paths or environment values. Native content capture
remains an explicit separate opt-in. Cancellation, deadlines, bounded output,
process groups and joined cleanup use the existing shell implementation.

This is executable/argv policy, not a semantic interpreter for git options or an
OS sandbox. A deny prefix `git push` matches `push` in the first argument position;
all other argv remain governed by the specified allowlist/default. An allowed
program can interpret its own inputs, run descendants or access the network.
Hosts own filesystem isolation and stable executable trees between check and
exec; canonical identity does not claim an atomic executable-file lease.

## Q01 observations

Run core and CLI/ACP fixtures for allow-only, deny-only, combined and independent
root/child layers; exact `git status --short` and prefix `git push`; quoting and
argument boundaries; symlink/hard-link/absolute/bare aliases; forbidden compound,
substitution, redirection, comment, newline and dynamic expansion; literal quoted
metacharacters; unknown/oversized config and parser input; environment defaults,
name restrictions and forbidden credentials; canonical cwd allow/deny/escape;
metadata privacy; cancellation and background cleanup. Confirm no process-start
event or marker effect for rejected inputs, and one existing terminal outcome.
Legacy suites must continue passing. Compare matched legacy/configured release
measurements and an explicitly restricted workload before merge.
