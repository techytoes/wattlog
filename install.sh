#!/bin/sh
set -eu

REPO="techytoes/wattlog"
ASSET_NAME="Wattlog-aarch64-apple-darwin.tar.gz"
SHA_NAME="${ASSET_NAME}.sha256"

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'wattlog install error: %s\n' "$*" >&2
  exit 1
}

# 1. Platform & architecture check
OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" != "Darwin" ]; then
  fail "Wattlog is a macOS utility (detected: $OS)."
fi
if [ "$ARCH" != "arm64" ]; then
  fail "Wattlog currently builds for Apple Silicon Macs (arm64); detected: $ARCH."
fi

command -v curl >/dev/null 2>&1 || fail "curl is required."
command -v tar >/dev/null 2>&1 || fail "tar is required."
command -v shasum >/dev/null 2>&1 || fail "shasum is required."

# 2. Resolve release version (honors WATTLOG_VERSION=v0.1.0, otherwise fetches latest release)
VERSION="${WATTLOG_VERSION:-}"
if [ -z "$VERSION" ]; then
  API_URL="https://api.github.com/repos/${REPO}/releases/latest"
  VERSION="$(curl -fsSL "$API_URL" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  if [ -z "$VERSION" ]; then
    fail "Could not resolve latest release from https://github.com/${REPO}/releases. Make sure the repository is public and has at least one published release."
  fi
fi

BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/wattlog-install.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

say "Installing Wattlog ${VERSION} (Apple Silicon)..."

# 3. Download tarball & SHA-256 checksum
curl -fL --progress-bar "${BASE_URL}/${ASSET_NAME}" -o "${TMP_DIR}/${ASSET_NAME}" \
  || fail "Failed to download ${BASE_URL}/${ASSET_NAME}"

if curl -fsSL "${BASE_URL}/${SHA_NAME}" -o "${TMP_DIR}/${SHA_NAME}"; then
  say "Verifying SHA-256 checksum..."
  (cd "$TMP_DIR" && shasum -a 256 -c "$SHA_NAME" >/dev/null) \
    || fail "SHA-256 checksum verification failed!"
fi

# 4. Extract bundle
tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "$TMP_DIR"
if [ ! -d "${TMP_DIR}/Wattlog.app" ]; then
  fail "Archive did not contain Wattlog.app."
fi

# 5. Choose install destination (/Applications if writable, otherwise ~/Applications — zero sudo)
if [ -w "/Applications" ]; then
  APP_DIR="/Applications"
else
  APP_DIR="${HOME}/Applications"
  mkdir -p "$APP_DIR"
fi
APP_DEST="${APP_DIR}/Wattlog.app"

# Gracefully stop existing Wattlog instance if running
if pgrep -x wattlog >/dev/null 2>&1; then
  say "Stopping running Wattlog instance..."
  pkill -x wattlog || true
  sleep 1
fi

rm -rf "$APP_DEST"
mv "${TMP_DIR}/Wattlog.app" "$APP_DEST"

# Clear quarantine attribute if present (safe safeguard)
xattr -dr com.apple.quarantine "$APP_DEST" >/dev/null 2>&1 || true

# 6. Symlink CLI binary (same binary supports --plain / --json / --demo)
BIN_TARGET="${APP_DEST}/Contents/MacOS/wattlog"
CLI_LINK=""
if [ -d "/opt/homebrew/bin" ] && [ -w "/opt/homebrew/bin" ]; then
  CLI_LINK="/opt/homebrew/bin/wattlog"
elif [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
  CLI_LINK="/usr/local/bin/wattlog"
else
  mkdir -p "${HOME}/.local/bin"
  CLI_LINK="${HOME}/.local/bin/wattlog"
fi

if [ -x "$BIN_TARGET" ]; then
  ln -sf "$BIN_TARGET" "$CLI_LINK"
fi

say ""
say "Installed Wattlog ${VERSION}:"
say "  App: ${APP_DEST}"
if [ -n "$CLI_LINK" ]; then
  say "  CLI: ${CLI_LINK}"
fi
say ""
say "Launch menu-bar app:"
say "  open -a Wattlog"
say ""
say "Quick terminal check (no GUI boot):"
say "  wattlog --plain"

case ":${PATH}:" in
  *":$(dirname "$CLI_LINK"):"*) ;;
  *)
    say ""
    say "Note: add $(dirname "$CLI_LINK") to your PATH to run 'wattlog' from any shell:"
    say "  export PATH=\"$(dirname "$CLI_LINK"):\$PATH\""
    ;;
esac
