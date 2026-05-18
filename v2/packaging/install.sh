#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
install.sh

Install or update llmattest on macOS/Linux.

Usage:
  curl -fsSL https://example.com/install.sh | sh
  LLMATTEST_RELEASE_BASE_URL=https://example.com/releases/latest/download v2/packaging/install.sh
  v2/packaging/install.sh --from ./llmattest-dev-darwin-arm64.tar.gz

Environment:
  LLMATTEST_RELEASE_BASE_URL  Base URL containing llmattest archives.
  LLMATTEST_VERSION           Version label in archive names. Default: latest.
  LLMATTEST_INSTALL_DIR       Install directory. Default: ~/.local/bin.
USAGE
}

INSTALL_DIR="${LLMATTEST_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${LLMATTEST_VERSION:-latest}"
BASE_URL="${LLMATTEST_RELEASE_BASE_URL:-}"
ARCHIVE_FROM=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --from)
      ARCHIVE_FROM="${2:-}"
      shift 2
      ;;
    --install-dir)
      INSTALL_DIR="${2:-}"
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

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os:$arch" in
  linux:x86_64) target="x86_64-unknown-linux-gnu" ;;
  linux:aarch64|linux:arm64) target="aarch64-unknown-linux-gnu" ;;
  darwin:x86_64) target="x86_64-apple-darwin" ;;
  darwin:arm64|darwin:aarch64) target="aarch64-apple-darwin" ;;
  *)
    echo "unsupported platform: $os/$arch" >&2
    exit 1
    ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [ -n "$ARCHIVE_FROM" ]; then
  archive="$ARCHIVE_FROM"
else
  if [ -z "$BASE_URL" ]; then
    echo "LLMATTEST_RELEASE_BASE_URL is required when --from is not used" >&2
    exit 1
  fi
  archive="$tmp/llmattest.tar.gz"
  url="${BASE_URL%/}/llmattest-${VERSION}-${target}.tar.gz"
  curl -fsSL "$url" -o "$archive"
fi

mkdir -p "$tmp/unpack" "$INSTALL_DIR"
tar -C "$tmp/unpack" -xzf "$archive"
bin="$(find "$tmp/unpack" -type f -name llmattest -perm -111 | head -1)"
if [ -z "$bin" ]; then
  echo "archive did not contain an executable llmattest binary" >&2
  exit 1
fi
install -m 0755 "$bin" "$INSTALL_DIR/llmattest"

echo "llmattest installed to $INSTALL_DIR/llmattest"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "add $INSTALL_DIR to PATH to run llmattest from any shell" ;;
esac
