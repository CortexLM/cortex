#!/bin/sh
# Install the Cortex subnet CLI (`ctx`) from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
#
# Knobs (all optional):
#   CTX_VERSION      release tag to install, e.g. vX.Y.Z (default: latest).
#                    Pin it to install a specific release, or when `latest`
#                    was published without ctx assets:
#                      curl -fsSL .../install-ctx.sh | CTX_VERSION=vX.Y.Z sh
#   CTX_INSTALL_DIR  install directory (default: $HOME/.local/bin)
#
# The ctx archives (ctx-<os>-<arch>.tar.gz) and SHA256SUMS.txt are attached
# to a release by the release-ctx GitHub Actions workflow
# (.github/workflows/release-ctx.yml). A release cut without that workflow has
# neither file; this script then stops and names the release instead of
# installing anything. Releases: https://github.com/CortexLM/cortex/releases
#
# The download is checksum-verified against the release's SHA256SUMS.txt. A
# missing or mismatched checksum aborts the install rather than running an
# unverified binary.

set -eu

REPO="CortexLM/cortex"
GATEWAY="https://gateway.cortex.foundation"
VERSION="${CTX_VERSION:-latest}"
INSTALL_DIR="${CTX_INSTALL_DIR:-$HOME/.local/bin}"
RELEASES="https://github.com/$REPO/releases"
SCRIPT_URL="https://raw.githubusercontent.com/$REPO/main/scripts/install-ctx.sh"

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
  *) die "unsupported OS $(uname -s). Windows users: download ctx-windows-amd64.zip from $RELEASES" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

asset="ctx-${os}-${arch}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  base="$RELEASES/latest/download"
else
  base="$RELEASES/download/$VERSION"
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

# Download $1 to $2. Returns 0 when fetched and 44 when the server answered
# 404 (that file is not on the release); any other failure aborts the install
# so a network error is never mistaken for a missing asset.
fetch() {
  code="$(curl -sSL -o "$2" -w '%{http_code}' "$1")" || die "download failed: $1"
  case "$code" in
    2??) return 0 ;;
    404)
      rm -f "$2"
      return 44
      ;;
    *) die "download failed (HTTP $code): $1" ;;
  esac
}

# The tag `latest` resolves to right now, so an error can name the release
# that is missing ctx assets. Best effort: a failure here only degrades the
# message, never the checksum verification.
release_label() {
  if [ "$VERSION" != "latest" ]; then
    echo "$VERSION"
    return 0
  fi
  url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$RELEASES/latest" 2>/dev/null || true)"
  case "$url" in
    */releases/tag/*) echo "${url##*/} (latest)" ;;
    *) echo "latest" ;;
  esac
}

# Whether $RELEASES/tag/<tag> exists at all, to tell "no such release" from
# "release exists but was cut without ctx assets". Only a 404 means absent;
# an outage or a network error aborts with its own message, so nobody is told
# to change CTX_VERSION when GitHub is what is failing.
release_exists() {
  case "$VERSION" in
    latest) return 0 ;;
  esac
  code="$(curl -sSLI -o /dev/null -w '%{http_code}' "$RELEASES/tag/$VERSION")" \
    || die "could not reach $RELEASES to check release $VERSION (network error); retry later"
  case "$code" in
    2??) return 0 ;;
    404) return 44 ;;
    *) die "HTTP $code from $RELEASES/tag/$VERSION while checking that the release exists; GitHub may be unavailable, retry later" ;;
  esac
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

label="$(release_label)"
echo "install-ctx: release $label"

# Sums first. A release published without the release-ctx workflow has no ctx
# assets at all, and that deserves a clearer message than a 404 on the tarball.
if ! fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt"; then
  if ! release_exists; then
    die "no release named $VERSION under $RELEASES (set CTX_VERSION to an existing tag, or unset it for latest)"
  fi
  cat >&2 <<EOF
install-ctx: release $label has no ctx assets (SHA256SUMS.txt is missing).
install-ctx: ctx archives are attached by the release-ctx GitHub Actions workflow;
install-ctx: this release was published without it. Refusing to install unverified.
install-ctx: Either wait for the operator to run release-ctx against that tag
install-ctx: (Actions -> release-ctx -> Run workflow -> tag=<that tag>), or pin a
install-ctx: release that lists ctx-*.tar.gz and SHA256SUMS.txt under $RELEASES:
install-ctx:   curl -fsSL $SCRIPT_URL | CTX_VERSION=vX.Y.Z sh
EOF
  exit 1
fi

want="$(grep " \{1,2\}\*\{0,1\}${asset}\$" "$tmp/SHA256SUMS.txt" | cut -d' ' -f1 | head -n1)"
[ -n "$want" ] || die "release $label has no ${os}-${arch} build ($asset is not listed in SHA256SUMS.txt)"

echo "install-ctx: downloading $asset"
fetch "$base/$asset" "$tmp/$asset" \
  || die "$asset is listed in SHA256SUMS.txt but missing from release $label (incomplete upload?)"
got="$(sha256_of "$tmp/$asset")"
[ "$want" = "$got" ] || die "checksum mismatch for $asset (expected $want, got $got)"

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/ctx" ] || die "$asset did not contain a ctx binary"

mkdir -p "$INSTALL_DIR"
cp "$tmp/ctx" "$INSTALL_DIR/ctx"
chmod 755 "$INSTALL_DIR/ctx"

echo "install-ctx: installed $("$INSTALL_DIR/ctx" --version) from release $label to $INSTALL_DIR/ctx"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "install-ctx: add it to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
echo "install-ctx: next steps"
echo "  ctx challenges        # the two live challenges and what they pay for"
echo "  ctx status            # whether each challenge can score right now"
echo "install-ctx: default gateway is $GATEWAY"
