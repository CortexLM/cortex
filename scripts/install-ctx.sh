#!/bin/sh
# Install the Cortex subnet CLI (`ctx`) from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
#
# Knobs (all optional):
#   CTX_VERSION      release tag to install, e.g. v0.2.0 (default: latest)
#   CTX_INSTALL_DIR  install directory (default: $HOME/.local/bin)
#
# The download is checksum-verified against the release's SHA256SUMS.txt. A
# missing or mismatched checksum aborts the install rather than running an
# unverified binary.

set -eu

REPO="CortexLM/cortex"
GATEWAY="https://network.cortex.foundation"
VERSION="${CTX_VERSION:-latest}"
INSTALL_DIR="${CTX_INSTALL_DIR:-$HOME/.local/bin}"

die() {
  echo "install-ctx: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) die "unsupported OS $(uname -s). Windows users: download ctx-windows-amd64.zip from https://github.com/$REPO/releases" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

asset="ctx-${os}-${arch}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "need sha256sum or shasum to verify the download"
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "install-ctx: downloading $asset ($VERSION)"
curl -fsSL "$base/$asset" -o "$tmp/$asset" \
  || die "download failed: $base/$asset"
curl -fsSL "$base/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt" \
  || die "no SHA256SUMS.txt in that release; refusing to install unverified"

want="$(grep " \{1,2\}\*\{0,1\}${asset}\$" "$tmp/SHA256SUMS.txt" | cut -d' ' -f1 | head -n1)"
[ -n "$want" ] || die "$asset is not listed in SHA256SUMS.txt"
got="$(sha256_of "$tmp/$asset")"
[ "$want" = "$got" ] || die "checksum mismatch for $asset (expected $want, got $got)"

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/ctx" ] || die "$asset did not contain a ctx binary"

mkdir -p "$INSTALL_DIR"
cp "$tmp/ctx" "$INSTALL_DIR/ctx"
chmod 755 "$INSTALL_DIR/ctx"

echo "install-ctx: installed $("$INSTALL_DIR/ctx" --version) to $INSTALL_DIR/ctx"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "install-ctx: add it to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
echo "install-ctx: next steps"
echo "  ctx challenges        # the four live challenges and what they pay for"
echo "  ctx status            # whether each challenge can score right now"
echo "install-ctx: default gateway is $GATEWAY"
