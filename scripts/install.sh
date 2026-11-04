#!/usr/bin/env bash
# Install the released wayfinder binary for this machine.
#
#   curl -fsSL https://raw.githubusercontent.com/zaterka/wayfinder/main/scripts/install.sh | bash
#
# Environment:
#   WAYFINDER_VERSION     release tag to install (default: latest)
#   WAYFINDER_INSTALL_DIR directory to install into (default: ~/.local/bin)
#   WAYFINDER_BASE_URL    release host override, for mirrors and tests
set -euo pipefail

REPO="${WAYFINDER_REPO:-zaterka/wayfinder}"
VERSION="${WAYFINDER_VERSION:-}"
INSTALL_DIR="${WAYFINDER_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${WAYFINDER_BASE_URL:-https://github.com/$REPO/releases}"

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Map this machine onto one released asset name.
asset_name() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os/$arch" in
    Linux/x86_64|Linux/amd64) printf 'wayfinder-linux-x86_64.tar.gz' ;;
    Linux/aarch64|Linux/arm64) printf 'wayfinder-linux-arm64.tar.gz' ;;
    Darwin/arm64) printf 'wayfinder-macos-arm64.tar.gz' ;;
    Darwin/x86_64) printf 'wayfinder-macos-x86_64.tar.gz' ;;
    *) die "no prebuilt binary for $os/$arch; build from source instead:
  cargo install --git https://github.com/$REPO wayfinder" ;;
  esac
}

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

ASSET="$(asset_name)"
if [ -n "$VERSION" ]; then
  URL="$BASE_URL/download/$VERSION/$ASSET"
else
  URL="$BASE_URL/latest/download/$ASSET"
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

log "downloading $ASSET"
if ! curl -fsSL "$URL" -o "$WORKDIR/$ASSET"; then
  die "could not download $URL
Either the release does not publish this platform yet, or there is no release:
  cargo install --git https://github.com/$REPO wayfinder"
fi

# Verify the release checksum when one was published beside the asset.
if curl -fsSL "$URL.sha256" -o "$WORKDIR/$ASSET.sha256" 2>/dev/null; then
  log "verifying checksum"
  ( cd "$WORKDIR" && shasum -a 256 -c "$ASSET.sha256" >/dev/null 2>&1 \
    || sha256sum -c "$ASSET.sha256" >/dev/null 2>&1 \
    || die "checksum mismatch for $ASSET" )
fi

log "unpacking"
tar -xzf "$WORKDIR/$ASSET" -C "$WORKDIR"

mkdir -p "$INSTALL_DIR"
installed=0
for binary in wayfinder wayfinder-bench; do
  found="$(find "$WORKDIR" -maxdepth 2 -type f -name "$binary" -print -quit)"
  [ -n "$found" ] || continue
  install -m 0755 "$found" "$INSTALL_DIR/$binary"
  installed=$((installed + 1))
done
[ "$installed" -gt 0 ] || die "the archive contained no wayfinder binary"

log "installed into $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '\nAdd it to your PATH:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" ;;
esac
printf '\nNext:\n  wayfinder init && wayfinder seed --structure && wayfinder install claude\n'
