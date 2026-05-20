#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
package-llmattest.sh

Build and package the llmattest one-binary participant client.

Usage:
  packaging/package-llmattest.sh [--target <triple>] [--version <version>] [--dist <dir>]

Examples:
  packaging/package-llmattest.sh
  packaging/package-llmattest.sh --target x86_64-unknown-linux-gnu
  packaging/package-llmattest.sh --target aarch64-apple-darwin --version 0.1.0

The script assumes the requested Rust target is already installed. CI should
install targets explicitly so release failures are easy to diagnose.
USAGE
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
TARGET=""
VERSION="${LLMATTEST_VERSION:-}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target)
      TARGET="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --dist)
      DIST_DIR="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$VERSION" ]; then
  VERSION="$(cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1 | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' | head -1)"
fi
if [ -z "$VERSION" ]; then
  VERSION="dev"
fi

if [ -n "$TARGET" ]; then
  cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --release --bin llmattest --target "$TARGET"
  TARGET_DIR="$ROOT_DIR/target/$TARGET/release"
  TARGET_NAME="$TARGET"
else
  cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --release --bin llmattest
  TARGET_DIR="$ROOT_DIR/target/release"
  TARGET_NAME="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
fi

BIN_NAME="llmattest"
ARCHIVE_EXT="tar.gz"
if printf '%s' "$TARGET_NAME" | grep -qi 'windows\|msvc\|mingw'; then
  BIN_NAME="llmattest.exe"
  ARCHIVE_EXT="zip"
fi

BIN_PATH="$TARGET_DIR/$BIN_NAME"
if [ ! -x "$BIN_PATH" ] && [ ! -f "$BIN_PATH" ]; then
  echo "missing built binary: $BIN_PATH" >&2
  exit 1
fi

PKG_NAME="llmattest-${VERSION}-${TARGET_NAME}"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir -p "$WORK_DIR/$PKG_NAME"
cp "$BIN_PATH" "$WORK_DIR/$PKG_NAME/$BIN_NAME"
cp "$ROOT_DIR/LLM_ATTESTED.md" "$WORK_DIR/$PKG_NAME/LLM_ATTESTED.md"
cat > "$WORK_DIR/$PKG_NAME/manifest.json" <<EOF
{
  "name": "llmattest",
  "version": "$VERSION",
  "target": "$TARGET_NAME",
  "binary": "$BIN_NAME",
  "entrypoint": "llmattest start --team-payload team.json -- <agent command>"
}
EOF

mkdir -p "$DIST_DIR"
if [ "$ARCHIVE_EXT" = "zip" ]; then
  (cd "$WORK_DIR" && zip -qr "$DIST_DIR/$PKG_NAME.zip" "$PKG_NAME")
  ARCHIVE_PATH="$DIST_DIR/$PKG_NAME.zip"
else
  tar -C "$WORK_DIR" -czf "$DIST_DIR/$PKG_NAME.tar.gz" "$PKG_NAME"
  ARCHIVE_PATH="$DIST_DIR/$PKG_NAME.tar.gz"
fi

if command -v shasum >/dev/null 2>&1; then
  (cd "$DIST_DIR" && shasum -a 256 "$(basename "$ARCHIVE_PATH")" > "$(basename "$ARCHIVE_PATH").sha256")
elif command -v sha256sum >/dev/null 2>&1; then
  (cd "$DIST_DIR" && sha256sum "$(basename "$ARCHIVE_PATH")" > "$(basename "$ARCHIVE_PATH").sha256")
else
  echo "warning: no SHA-256 tool found; checksum not written" >&2
fi

echo "$ARCHIVE_PATH"
