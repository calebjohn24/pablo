---
title: Installation and release files
description: Install an exact Pablo release, inspect its checksums and manifests, and understand platform and signing requirements.
---

# Installation and release files

Pablo releases use four versioned archives, one for each supported operating-system and architecture pair. The installer chooses the target from `uname`, verifies the archive before extraction, executes the binary’s `--version`, and writes only the executable and an ownership receipt beneath an absolute prefix.

## Supported targets

| Target | Runtime requirement | Distribution limit |
| --- | --- | --- |
| `aarch64-apple-darwin` | macOS 11.0 or newer on Apple silicon | Unsigned and not notarized |
| `x86_64-apple-darwin` | macOS 10.15 or newer on Intel | Unsigned and not notarized |
| `aarch64-unknown-linux-gnu` | 64-bit Linux with glibc 2.35 or newer | GNU libc build; musl is not supported |
| `x86_64-unknown-linux-gnu` | 64-bit Linux with glibc 2.35 or newer | GNU libc build; musl is not supported |

The macOS deployment targets are set explicitly during compilation. Linux archives are built natively on Ubuntu 22.04 runners and declare glibc 2.35 as the minimum. Each target manifest records the build runner, dynamic-library report, runtime requirements, signing state and binary hash.

## Install an exact version

Install the native Mac or Linux binary with one command:

```sh
curl -fsSL https://runpablo.pages.dev/install.sh | sh
```

The published script pins `v0.0.1`, installs to `$HOME/.local/bin/pablo`, and never resolves a moving `latest` version. Repeating the command is a successful no-op only when the same version, target, receipt and binary hash still match. Add `$HOME/.local/bin` to your shell's `PATH` when needed. To review the installer before running it, open [the published script](https://runpablo.pages.dev/install.sh) or [its repository source](https://github.com/calebjohn24/pablo/blob/main/install.sh).

Override either default while keeping a one-line command:

```sh
curl -fsSL https://runpablo.pages.dev/install.sh | sh -s -- --version v0.0.1 --prefix "$HOME/.local"
```

The prefix must be absolute. The installer downloads the matching archive and `.sha256` file from `https://runpablo.pages.dev/releases/<version>/`, backed by a private Cloudflare R2 bucket through a read-only Pages Function.

Installation fails before changing the prefix when:

- the archive or checksum cannot be downloaded over HTTPS;
- the checksum is malformed or does not match;
- the archive does not contain the exact versioned `bin/pablo` member;
- the binary does not execute or reports another version;
- another `pablo` command is visible outside the selected prefix; or
- `$PREFIX/bin/pablo` already exists.

Pablo never invokes `sudo`, a package manager, Node, or Python. Add `$HOME/.local/bin` to `PATH` yourself when that is your chosen prefix.

## Update, replacement and removal

Update to another exact version with the same inspected installer:

```sh
curl -fsSL https://runpablo.pages.dev/install.sh | sh -s -- --version v0.0.2 --update
```

`--update` requires an existing receipt from this installer, checks the installed binary hash and target, and refuses the same version or a locally changed executable. The new archive still passes download, checksum and version validation before replacement. If receipt installation fails, the previous binary is restored.

Use `--replace` for the broader explicit policy that replaces any regular, non-symlink `pablo` file at the selected path. It does not require an earlier receipt, so inspect the target yourself before choosing it.

An installation receipt at `$PREFIX/share/pablo/install-receipt` records the version, target, binary path, and binary SHA-256. Removal checks that receipt and refuses to delete a binary whose bytes changed:

```sh
curl -fsSL https://runpablo.pages.dev/install.sh | sh -s -- --remove
```

Removal leaves unrelated files and directories beneath the prefix in place.

## Verify a download manually

Every archive has an adjacent checksum file, and the release set includes `SHA256SUMS`:

```sh
version=v0.0.1
target=aarch64-apple-darwin
base="https://runpablo.pages.dev/releases/$version"
archive="pablo-$version-$target.tar.gz"

curl -fsSLO "$base/$archive"
curl -fsSLO "$base/$archive.sha256"
shasum -a 256 -c "$archive.sha256"
tar -tzf "$archive"
```

On Linux, use `sha256sum -c` instead of `shasum -a 256 -c` when preferred.

## Archive contents

The archive has one versioned root and never writes outside it:

```text
pablo-v0.0.1-aarch64-apple-darwin/
├── bin/pablo
├── LICENSE
├── NOTICE
├── THIRD_PARTY_NOTICES.json
├── licenses/<license-content-sha256>.txt
└── manifest/
    ├── source.json
    ├── build.json
    └── target.json
```

`LICENSE` contains Pablo’s MIT license. `source.json` records the Git commit and tree, source fingerprint, `Cargo.lock` hash, version, and source epoch. `build.json` records the locked release command, Rust, Cargo and Node identities, runner, binary hash, and deterministic archive settings. `target.json` records the target triple, platform requirements, dynamic libraries, signing state, and native-acceptance status.

The packer writes lexically ordered ustar entries with fixed ownership and timestamps, then creates a gzip stream with a zero timestamp. Repackaging identical inputs produces identical archive bytes. The third-party inventory comes from the target-filtered locked Cargo dependency closure; bundled license and notice files are deduplicated by content hash while package attribution remains machine-readable.

## Build from source

Source builds use Rust `1.98.1` and the checked-in lockfile:

```sh
git clone https://github.com/calebjohn24/pablo.git
cd pablo
cargo build --release --locked -p pablo --bin pablo
./target/release/pablo --version
```

The release profile enables thin LTO, uses one codegen unit, and strips symbols. Source builds are distinct from the published archives because they do not carry the archive’s source, build and target identities.
