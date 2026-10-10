#!/usr/bin/env bash
# Mirror of the Cortex supervisor label check (cortex@d738424 supervisor.py verify_labels) plus
# the contract-v1 user/port rules. Exit 0 = conformant; 1 = refused (reasons on stderr).
#   scripts/verify_image.sh IMAGE [SLUG] [SOURCE]      ENGINE=docker|podman (default podman)
set -euo pipefail
if [ "${1:-}" = --cuda-candidate ]; then
  image=${2:?candidate image required}
  manifest=${3:?source manifest required}
  revision=${4:?Git revision required}
  engine=${ENGINE:-podman}
  python3 - "$manifest" <<'PY'
import hashlib
import json
import pathlib
import sys

expected = json.loads(pathlib.Path(sys.argv[1]).read_text())
root = pathlib.Path.cwd()
actual = {
    str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
    for path in (root / 'src/hypertrain').rglob('*')
    if path.is_file() and '__pycache__' not in path.parts and path.suffix != '.pyc'
}
assert actual == expected['source_files'], 'source changed after candidate freeze'
assert all(hashlib.sha256((root / name).read_bytes()).hexdigest() == digest for name, digest in expected['copy_files'].items())
PY
  config=$("$engine" image inspect "$image" --format '{{json .Config}}')
  test "$(jq -r '.Labels["org.opencontainers.image.revision"]' <<<"$config")" = "$revision"
  test "$(jq -r '.Labels["io.cortex.challenge.source-manifest"]' <<<"$config")" = "$(sha256sum "$manifest" | cut -d ' ' -f 1)"
  test "$(jq -c '.Entrypoint' <<<"$config")" = '["hypertrain-miner"]'
  "$engine" run --rm -i --network=none --read-only \
    --tmpfs /tmp:rw,nosuid,nodev,mode=1777 \
    -e PYTHONDONTWRITEBYTECODE=1 -e OMP_NUM_THREADS=1 -e MKL_NUM_THREADS=1 \
    -v "$(realpath "$manifest"):/candidate-source.json:ro" \
    --entrypoint /opt/hypertrain/venv/bin/python "$image" - <<'PY'
import hashlib
import importlib.metadata
import json
import pathlib
import subprocess
import tomllib

import hypertrain
import torch

expected = json.loads(pathlib.Path('/candidate-source.json').read_text())
package = pathlib.Path(hypertrain.__file__).resolve().parent
assert package == pathlib.Path('/opt/hypertrain/venv/lib/python3.12/site-packages/hypertrain')
actual = {
    'src/hypertrain/' + str(path.relative_to(package)): hashlib.sha256(path.read_bytes()).hexdigest()
    for path in package.rglob('*')
    if path.is_file() and '__pycache__' not in path.parts and path.suffix != '.pyc'
}
assert actual == expected['source_files'], 'installed package differs from frozen source'
packages = {entry['name']: entry for entry in tomllib.loads(pathlib.Path('/opt/hypertrain/src/uv.lock').read_text())['package']}
pending = [entry['name'] for entry in packages['hypertrain']['dependencies']]
versions = {}
while pending:
    name = pending.pop()
    if name in versions:
        continue
    entry = packages[name]
    versions[name] = entry['version']
    pending.extend(dependency['name'] for dependency in entry.get('dependencies', []))
versions.update(expected['backend_versions'])
assert len(versions) == 39, 'review required for changed dependency set'
assert {name: importlib.metadata.version(name) for name in versions} == versions
assert torch.__version__ == '2.14.0+cu130'
od = json.loads(importlib.metadata.distribution('opendecision').read_text('direct_url.json'))
assert od['url'] == 'https://github.com/CortexLM/opendecision/archive/e125ff756d57b726fbce44c430e3d2207e7bbda8.tar.gz'
scripts = {entry.name: entry.value for entry in importlib.metadata.distribution('hypertrain').entry_points if entry.group == 'console_scripts'}
assert scripts == {'hypertrain': 'hypertrain.aggregator.cli:main', 'hypertrain-miner': 'hypertrain.miner.cli:main'}
for name in scripts:
    executable = pathlib.Path('/opt/hypertrain/venv/bin') / name
    assert executable.read_text().splitlines()[0] == '#!/opt/hypertrain/venv/bin/python'
    subprocess.run([str(executable), '--help'], check=True, timeout=30)
import hypertrain.trainer
import hypertrain.auditor.__main__
import hypertrain.models.opendecision
print(json.dumps({'source_files': actual, 'versions': versions, 'package': str(package), 'torch': torch.__version__, 'entrypoints': scripts}, sort_keys=True))
PY
  exit 0
fi
image=${1:?usage: verify_image.sh IMAGE [SLUG] [SOURCE]}
slug=${2:-hypertrain}
source=${3:-https://github.com/CortexLM/hypertrain}
engine=${ENGINE:-podman}
config=$("$engine" image inspect "$image" --format '{{json .Config}}')
fail=0
check() { # name actual expected
  if [ "$2" != "$3" ]; then echo "REFUSED: $1 is '$2', expected '$3'" >&2; fail=1; fi
}
label() { jq -r --arg k "$1" '(.Labels // {})[$k] // ""' <<<"$config"; }
check io.cortex.challenge.slug "$(label io.cortex.challenge.slug)" "$slug"
check io.cortex.challenge.contract "$(label io.cortex.challenge.contract)" "1"
src=$(label org.opencontainers.image.source)
check org.opencontainers.image.source "${src%/}" "$source"
check User "$(jq -r '.User // ""' <<<"$config")" "65532:65532"
check ExposedPorts "$(jq -r '(.ExposedPorts // {}) | has("8000/tcp")' <<<"$config")" "true"
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "OK: $image labels slug=$slug contract=1 source=$source user=65532:65532 port=8000"
