#!/usr/bin/env bash
# Seedless, idempotent miner install from a signed release wheel. Never prompts for a seed.
# Usage: install-miner.sh MANIFEST_URL [BUNDLE_DIR]
#   MANIFEST_URL  signed release manifest (verified against the pinned release key)
#   BUNDLE_DIR    server-key bundle from `hypertrain-miner key create` (optional)
set -euo pipefail
manifest_url=${1:?usage: install-miner.sh MANIFEST_URL [BUNDLE_DIR]}
bundle=${2:-}
root=${HYPERTRAIN_UPDATE_ROOT:-$HOME/.hypertrain/miner}
mkdir -p "$root" && chmod 700 "$root"
exec </dev/null  # no interactive input, ever

command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"

# Bootstrap venv: needed only to verify the manifest signature (pinned key lives in the package).
boot="$root/bootstrap"
manifest=$(curl -fsSL "$manifest_url")
wheel_url=$(printf '%s' "$manifest" | uv run --no-project python -c 'import json,sys; print(json.load(sys.stdin)["manifest"]["wheel_url"])')
version=$(printf '%s' "$manifest" | uv run --no-project python -c 'import json,sys; print(json.load(sys.stdin)["manifest"]["version"])')
dest="$root/versions/$version"
if [[ ! -x "$dest/venv/bin/hypertrain-miner" ]]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL -o "$tmp/$(basename "$wheel_url")" "$wheel_url"
  [[ -d "$boot" ]] || uv venv -q "$boot"
  uv pip install -q --python "$boot/bin/python" "$tmp/$(basename "$wheel_url")"
  printf '%s' "$manifest" >"$tmp/manifest.json"
  # Verifies signature + wheel sha256 before anything is trusted; exits nonzero otherwise.
  "$boot/bin/python" - "$tmp" <<'EOF'
import hashlib, sys
from pathlib import Path
from hypertrain.miner.update import release_pubkey, verify_manifest, wheel_name
tmp = Path(sys.argv[1])
m = verify_manifest((tmp / "manifest.json").read_bytes(), release_pubkey())
if hashlib.sha256((tmp / wheel_name(m)).read_bytes()).hexdigest() != m.wheel_sha256:
    sys.exit("wheel sha256 mismatch")
EOF
  rm -rf "$dest"
  mkdir -p "$dest"
  uv venv -q "$dest/venv"
  uv pip install -q --python "$dest/venv/bin/python" "$tmp/$(basename "$wheel_url")"
  printf '%s' "$(sha256sum "$tmp/$(basename "$wheel_url")" | cut -d' ' -f1)" >"$dest/STAGED"
fi
ln -sfn "versions/$version" "$root/.current.tmp" && mv -Tf "$root/.current.tmp" "$root/current"

if [[ -n "$bundle" && ! -f "$root/miner.toml" ]]; then
  "$root/current/venv/bin/hypertrain-miner" install --bundle "$bundle" --config "$root/miner.toml"
fi
echo "hypertrain-miner $version installed at $root/current"
