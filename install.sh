#!/bin/sh

set -eu

PROGRAM="pablo"
VERSION=""
PREFIX=""
ARCHIVE_PATH=""
CHECKSUM_PATH=""
DOWNLOAD_BASE="https://runpablo.pages.dev/releases"
REPLACE=0
UPDATE=0
REMOVE=0
WORKDIR=""
STAGED_BINARY=""
STAGED_RECEIPT=""
BACKUP_BINARY=""

usage() {
  cat <<'EOF'
Install or remove a pinned Pablo release.

Usage:
  install.sh --version TAG --prefix ABSOLUTE_PATH [--replace]
  install.sh --version TAG --prefix ABSOLUTE_PATH --update
  install.sh --version TAG --prefix ABSOLUTE_PATH \
    --archive FILE --checksum FILE [--replace]
  install.sh --prefix ABSOLUTE_PATH --remove

Options:
  --version TAG         Exact GitHub release tag, for example v0.1.0-dev.1.
  --prefix PATH         Absolute installation prefix. The binary goes in PATH/bin.
  --archive FILE        Install a local release archive instead of downloading it.
  --checksum FILE       SHA-256 file for --archive; required with --archive.
  --download-base URL   Alternate HTTPS release root containing TAG/archive files.
  --replace             Explicitly replace PATH/bin/pablo if it is a regular file.
  --update              Upgrade an unchanged, receipt-backed Pablo installation.
  --remove              Remove an unchanged, receipt-backed installation.
  -h, --help            Show this help.

The installer never invokes sudo or a package manager. It refuses commands found
outside the selected prefix and never silently overwrites an existing executable.
EOF
}

die() {
  printf 'pablo installer: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$STAGED_BINARY" ]; then
    rm -f "$STAGED_BINARY"
  fi
  if [ -n "$STAGED_RECEIPT" ]; then
    rm -f "$STAGED_RECEIPT"
  fi
  if [ -n "$BACKUP_BINARY" ]; then
    rm -f "$BACKUP_BINARY"
  fi
  if [ -n "$WORKDIR" ]; then
    rm -rf "$WORKDIR"
  fi
}

trap cleanup EXIT HUP INT TERM

need_value() {
  [ "$#" -ge 2 ] || die "$1 requires a value"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      need_value "$@"
      VERSION=$2
      shift 2
      ;;
    --prefix)
      need_value "$@"
      PREFIX=$2
      shift 2
      ;;
    --archive)
      need_value "$@"
      ARCHIVE_PATH=$2
      shift 2
      ;;
    --checksum)
      need_value "$@"
      CHECKSUM_PATH=$2
      shift 2
      ;;
    --download-base)
      need_value "$@"
      DOWNLOAD_BASE=$2
      shift 2
      ;;
    --replace)
      REPLACE=1
      shift
      ;;
    --update)
      UPDATE=1
      shift
      ;;
    --remove)
      REMOVE=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

[ -n "$PREFIX" ] || die "--prefix is required"
case "$PREFIX" in
  /*) ;;
  *) die "--prefix must be an absolute path" ;;
esac
while [ "$PREFIX" != "/" ] && [ "${PREFIX%/}" != "$PREFIX" ]; do
  PREFIX=${PREFIX%/}
done

if [ "$PREFIX" = "/" ]; then
  PREFIX_ROOT=""
else
  PREFIX_ROOT=$PREFIX
fi

BIN_DIR="${PREFIX_ROOT}/bin"
SHARE_DIR="${PREFIX_ROOT}/share/pablo"
TARGET_PATH="${BIN_DIR}/${PROGRAM}"
RECEIPT_PATH="${SHARE_DIR}/install-receipt"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  else
    die "SHA-256 verification requires sha256sum, shasum, or openssl"
  fi
}

receipt_value() {
  sed -n "s/^$1=//p" "$RECEIPT_PATH"
}

verify_owned_install() {
  ACTION=$1
  [ ! -L "$RECEIPT_PATH" ] && [ -f "$RECEIPT_PATH" ] || die "no valid install receipt at $RECEIPT_PATH"
  [ ! -L "$TARGET_PATH" ] && [ -f "$TARGET_PATH" ] || die "no regular installed binary at $TARGET_PATH"

  RECEIPT_FORMAT=$(receipt_value format)
  RECEIPT_VERSION=$(receipt_value version)
  RECEIPT_TARGET=$(receipt_value target)
  RECEIPT_BINARY=$(receipt_value binary)
  RECEIPT_HASH=$(receipt_value sha256 | tr 'A-F' 'a-f')
  [ "$RECEIPT_FORMAT" = "1" ] || die "unsupported install receipt at $RECEIPT_PATH"
  [ "$RECEIPT_BINARY" = "bin/pablo" ] || die "install receipt does not own $TARGET_PATH"
  [ -n "$RECEIPT_VERSION" ] && [ -n "$RECEIPT_TARGET" ] || die "incomplete install receipt at $RECEIPT_PATH"
  [ "${#RECEIPT_HASH}" -eq 64 ] || die "invalid binary hash in install receipt"
  case "$RECEIPT_HASH" in
    *[!0-9a-f]*) die "invalid binary hash in install receipt" ;;
  esac

  ACTUAL_HASH=$(sha256_file "$TARGET_PATH" | tr 'A-F' 'a-f')
  [ "$ACTUAL_HASH" = "$RECEIPT_HASH" ] || die "installed binary changed; refusing to $ACTION it"
}

[ "$REPLACE" -eq 0 ] || [ "$UPDATE" -eq 0 ] || die "--replace and --update cannot be combined"

if [ "$REMOVE" -eq 1 ]; then
  [ "$REPLACE" -eq 0 ] || die "--remove cannot be combined with --replace"
  [ "$UPDATE" -eq 0 ] || die "--remove cannot be combined with --update"
  [ -z "$ARCHIVE_PATH" ] || die "--remove cannot be combined with --archive"
  [ -z "$CHECKSUM_PATH" ] || die "--remove cannot be combined with --checksum"
  verify_owned_install remove

  rm -f "$TARGET_PATH"
  rm -f "$RECEIPT_PATH"
  rmdir "$SHARE_DIR" 2>/dev/null || :
  printf 'Removed %s\n' "$TARGET_PATH"
  exit 0
fi

[ -n "$VERSION" ] || die "--version is required for installation"
case "$VERSION" in
  *[!A-Za-z0-9._-]*|'') die "--version contains unsupported characters" ;;
esac
VERSION_NUMBER=${VERSION#v}
[ -n "$VERSION_NUMBER" ] || die "--version must include a version number"

if [ -n "$ARCHIVE_PATH" ] || [ -n "$CHECKSUM_PATH" ]; then
  [ -n "$ARCHIVE_PATH" ] && [ -n "$CHECKSUM_PATH" ] || die "--archive and --checksum must be supplied together"
  [ -f "$ARCHIVE_PATH" ] || die "archive not found: $ARCHIVE_PATH"
  [ -f "$CHECKSUM_PATH" ] || die "checksum file not found: $CHECKSUM_PATH"
else
  case "$DOWNLOAD_BASE" in
    https://*) ;;
    *) die "--download-base must use HTTPS" ;;
  esac
fi

SYSTEM=$(uname -s)
MACHINE=$(uname -m)
case "$SYSTEM:$MACHINE" in
  Darwin:arm64|Darwin:aarch64) TARGET="aarch64-apple-darwin" ;;
  Darwin:x86_64|Darwin:amd64) TARGET="x86_64-apple-darwin" ;;
  Linux:aarch64|Linux:arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  Linux:x86_64|Linux:amd64) TARGET="x86_64-unknown-linux-gnu" ;;
  *) die "unsupported platform: $SYSTEM $MACHINE" ;;
esac

ARCHIVE_NAME="pablo-${VERSION}-${TARGET}.tar.gz"
ARCHIVE_ROOT="pablo-${VERSION}-${TARGET}"

VISIBLE_COMMAND=$(command -v "$PROGRAM" 2>/dev/null || :)
if [ -n "$VISIBLE_COMMAND" ] && [ "$VISIBLE_COMMAND" != "$TARGET_PATH" ]; then
  die "an existing pablo command resolves outside the selected prefix: $VISIBLE_COMMAND"
fi

if [ "$UPDATE" -eq 1 ]; then
  verify_owned_install update
  [ "$RECEIPT_TARGET" = "$TARGET" ] || die "installed target $RECEIPT_TARGET does not match this host ($TARGET)"
  [ "$RECEIPT_VERSION" != "$VERSION" ] || die "pablo $VERSION_NUMBER is already installed at $TARGET_PATH"
  REPLACE=1
fi

TARGET_EXISTS=0
if [ -e "$TARGET_PATH" ] || [ -L "$TARGET_PATH" ]; then
  TARGET_EXISTS=1
  [ "$REPLACE" -eq 1 ] || die "$TARGET_PATH already exists; pass --replace to replace it explicitly"
  [ ! -L "$TARGET_PATH" ] && [ -f "$TARGET_PATH" ] || die "refusing to replace a non-regular file at $TARGET_PATH"
fi

WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/pablo-install.XXXXXX") || die "could not create a temporary directory"
mkdir -p "$WORKDIR/extract"

if [ -z "$ARCHIVE_PATH" ]; then
  command -v curl >/dev/null 2>&1 || die "curl is required to download a release"
  ARCHIVE_PATH="$WORKDIR/$ARCHIVE_NAME"
  CHECKSUM_PATH="$WORKDIR/$ARCHIVE_NAME.sha256"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$ARCHIVE_PATH" "$DOWNLOAD_BASE/$VERSION/$ARCHIVE_NAME" || die "could not download $ARCHIVE_NAME"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$CHECKSUM_PATH" "$DOWNLOAD_BASE/$VERSION/$ARCHIVE_NAME.sha256" || die "could not download $ARCHIVE_NAME.sha256"
fi

EXPECTED_HASH=$(awk 'NF {print $1; exit}' "$CHECKSUM_PATH" | tr 'A-F' 'a-f')
[ "${#EXPECTED_HASH}" -eq 64 ] || die "checksum file does not contain one SHA-256 digest"
case "$EXPECTED_HASH" in
  *[!0-9a-f]*) die "checksum file contains an invalid SHA-256 digest" ;;
esac

ACTUAL_ARCHIVE_HASH=$(sha256_file "$ARCHIVE_PATH" | tr 'A-F' 'a-f')
[ "$ACTUAL_ARCHIVE_HASH" = "$EXPECTED_HASH" ] || die "checksum verification failed for $ARCHIVE_NAME"

tar -xzf "$ARCHIVE_PATH" -C "$WORKDIR/extract" "$ARCHIVE_ROOT/bin/pablo" 2>/dev/null || die "archive does not contain $ARCHIVE_ROOT/bin/pablo"
CANDIDATE="$WORKDIR/extract/$ARCHIVE_ROOT/bin/pablo"
[ ! -L "$CANDIDATE" ] && [ -f "$CANDIDATE" ] || die "archive binary is not a regular file"
chmod 755 "$CANDIDATE"

VERSION_OUTPUT=$("$CANDIDATE" --version 2>&1) || die "archive binary did not run on $TARGET"
case "$VERSION_OUTPUT" in
  "pablo $VERSION_NUMBER"|"pablo $VERSION_NUMBER "*) ;;
  *) die "archive binary version mismatch: expected pablo $VERSION_NUMBER" ;;
esac
BINARY_HASH=$(sha256_file "$CANDIDATE" | tr 'A-F' 'a-f')

[ ! -L "$BIN_DIR" ] || die "refusing symlinked binary directory: $BIN_DIR"
[ ! -L "$SHARE_DIR" ] || die "refusing symlinked receipt directory: $SHARE_DIR"
mkdir -p "$BIN_DIR" "$SHARE_DIR"

STAGED_BINARY=$(mktemp "$BIN_DIR/.pablo.install.XXXXXX") || die "could not stage the binary in $BIN_DIR"
cp "$CANDIDATE" "$STAGED_BINARY"
chmod 755 "$STAGED_BINARY"

STAGED_RECEIPT=$(mktemp "$SHARE_DIR/.install-receipt.XXXXXX") || die "could not stage the install receipt"
cat >"$STAGED_RECEIPT" <<EOF
format=1
version=$VERSION
target=$TARGET
sha256=$BINARY_HASH
binary=bin/pablo
EOF
chmod 644 "$STAGED_RECEIPT"

if [ "$TARGET_EXISTS" -eq 1 ]; then
  [ ! -L "$TARGET_PATH" ] && [ -f "$TARGET_PATH" ] || die "installation target changed before replacement"
  BACKUP_BINARY=$(mktemp "$BIN_DIR/.pablo.backup.XXXXXX") || die "could not stage a replacement backup"
  cp "$TARGET_PATH" "$BACKUP_BINARY"
  chmod 755 "$BACKUP_BINARY"
  mv -f "$STAGED_BINARY" "$TARGET_PATH"
  STAGED_BINARY=""
else
  ln "$STAGED_BINARY" "$TARGET_PATH" 2>/dev/null || die "$TARGET_PATH appeared during installation; nothing was overwritten"
  rm -f "$STAGED_BINARY"
  STAGED_BINARY=""
fi

if ! mv -f "$STAGED_RECEIPT" "$RECEIPT_PATH"; then
  if [ -n "$BACKUP_BINARY" ]; then
    mv -f "$BACKUP_BINARY" "$TARGET_PATH"
    BACKUP_BINARY=""
  else
    rm -f "$TARGET_PATH"
  fi
  die "could not install the receipt; the binary change was rolled back"
fi
STAGED_RECEIPT=""
if [ -n "$BACKUP_BINARY" ]; then
  rm -f "$BACKUP_BINARY"
  BACKUP_BINARY=""
fi

printf 'Installed pablo %s to %s\n' "$VERSION_NUMBER" "$TARGET_PATH"
printf 'Add %s to PATH if it is not already present.\n' "$BIN_DIR"
