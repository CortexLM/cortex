#!/usr/bin/env bash
# Swap the image of the s100 TEST challenge, retune the TEST beacon pusher, and roll both back.
# Never touches the production challenge or the master; the TEST relay config and stub metagraph
# are only put back by rollback, from the copies backup took.
# Run on s100 over stdin: ssh root@s100 'bash -s -- CMD ARGS' < challenge_image.sh
#   backup TS            snapshot compose files, both SQLite DBs (integrity ok), relay.json,
#                        metagraph.json and inspect JSON (env names only), with SHA256SUMS
#   swap NEW_REF BACKUP  gate, pin NEW_REF on the challenge service, up -d --no-deps challenge
#   beacon BACKUP        gate, set the pusher to --interval 0.2 --catch-up 3 (the production
#                        twin's values; 3/20 holds the lag above the freshness bound of 2 rounds)
#   paper BACKUP         dry rollback: the backed-up compose file must parse and resolve the old image
#   rollback BACKUP      restore the compose file, recreate challenge and beacon as they were, and
#                        put back relay.json / metagraph.json (restarting relay / stub) if changed
#   restore-db BACKUP    keep the live DBs aside, stop the challenge, put the backed-up DBs back
#   executor BACKUP PATH  gate, add HYPERTRAIN_CUDA_EXECUTOR=PATH (a container path) to the challenge
#                        through a compose override file; only that variable may change
#   executor-off BACKUP  drop the override and recreate the challenge without the variable
# rollback and restore-db are not gated: they are the way back from a bad change.
# swap, beacon, paper and rollback refuse (exit 4) a compose file with a service whose container_name
# is missing or is not one of the six TEST names; restore-db runs no compose.
set -euo pipefail
T=/root/backups/hypertrain-test; F=$T/compose.test.yml; C=hypertrain-test-challenge
STATE=/data/hypertrain-test/state; API=http://127.0.0.1:18996; MASTER=http://10.177.245.39:8080
cmd=${1:?backup|swap|paper|rollback|restore-db|executor|executor-off}
compose() { (cd "$T" && docker compose -f "$1" "${@:2}"); }
images() { # service -> image as compose resolves it
  compose "$1" config --format json | python3 -I -c '
import json,sys
for n,s in sorted(json.load(sys.stdin)["services"].items()): print(n, s["image"])'; }
# compose up acts on the container_name values of the file, so every service of a file about to be
# brought up must declare one of these: a name one token away is a production container
TEST_NAMES="hypertrain-test-challenge hypertrain-test-beacon hypertrain-test-master-stub
  hypertrain-test-relay hypertrain-test-auditor-0 hypertrain-test-auditor-1"
test_names() { # test_names FILE < compose config JSON of FILE
  python3 -I -c '
import json,sys
ok=sys.argv[2].split(); bad=0; svc=json.load(sys.stdin)["services"]
for n,s in sorted(svc.items()):
    c=s.get("container_name")
    if c not in ok:
        bad=1; print("REFUSED %s: service %s %s" % (sys.argv[1], n, "names container "+c if c else "declares no container_name"))
# up -d --no-deps challenge beacon names these two services: compose config leaves out a service whose
# profile is off, which up would still start, and a TEST name on the wrong service is no better
for n,c in (("challenge","hypertrain-test-challenge"),("beacon","hypertrain-test-beacon")):
    if svc.get(n,{}).get("container_name")!=c:
        bad=1; print("REFUSED %s: service %s must be %s" % (sys.argv[1], n, c))
sys.exit(bad)' "$1" "$TEST_NAMES"; }
backup_names() { # backup_names BACKUP: its compose file, which rollback copies over the live one
  # compose() runs in $T, so a relative BACKUP must be made absolute first
  local cfg f; f=$(cd "$1" && pwd)/compose.test.yml
  cfg=$(compose "$f" config --format json) && test_names "$f" <<< "$cfg" || exit 4; }
gate() { # readyz 200, no seal backlog, and a sealed seal younger than 30 min (first half of the
  # 72-min window; a seal delayed by unfinished work would make "young" mean "late", so the
  # sealed epoch must also be the one just before the master's current epoch)
  local ready; ready=$(curl -sf -m 10 "$MASTER/readyz") || { echo "GATE readyz not 200"; exit 2; }
  curl -s -m 10 "$MASTER/v1/weights/latest" | python3 -I -c '
import json,sys,datetime
d=json.load(sys.stdin); c=datetime.datetime.fromisoformat(d["computed_at"].replace("Z","+00:00"))
age=(datetime.datetime.now(datetime.timezone.utc)-c).total_seconds(); now=json.loads(sys.argv[1])["epoch"]
print("gate seal epoch",d["epoch"],"master epoch",now,"sealed",d.get("sealed"),"age_s",int(age))
sys.exit(0 if d.get("sealed") is True and 0<=age<1800 and d["epoch"]==now-1 else 3)' "$ready" \
    || { echo "GATE seal not sealed, older than 30 min, or behind"; exit 3; }; }
live() { # live IMAGE_ID: the challenge answers /version and runs exactly IMAGE_ID
  local i; for i in $(seq 1 30); do curl -sf -m 5 -o /dev/null "$API/version" && break; sleep 1; done
  curl -sf -m 5 -o /dev/null "$API/version" || { echo "challenge /version not 2xx"; exit 7; }
  [ "$(docker inspect "$C" --format '{{.Image}}')" = "$1" ] || { echo "challenge not on $1"; exit 7; }; }
check() { # what an operator reads after any change
  docker inspect "$C" --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}} restarts={{.RestartCount}}'
  for p in version health public/v1/runs; do
    printf '%s ' "$p"; curl -s -m 10 -w ' %{http_code}\n' "$API/$p" | cut -c1-200; done
  printf 'public runs sha '; curl -s -m 10 "$API/public/v1/runs" | sha256sum | cut -c1-64; }
apply() { # apply STEP BACKUP PATH..: $F.new must differ from $F in exactly PATH.., then go live
  local step=$1 B=$2; shift 2
  compose "$F" config --format json > "$B/config-before-$step.json"
  compose "$F.new" config --format json > "$B/config-after-$step.json"
  { test_names "$F" < "$B/config-before-$step.json" && test_names "$F.new" < "$B/config-after-$step.json"; } \
    || { rm -f "$F.new"; exit 4; }
  diff "$F" "$F.new" || true
  python3 -I - "$B/config-before-$step.json" "$B/config-after-$step.json" "$@" <<'EOF' \
    || { rm -f "$F.new"; echo "unexpected compose change"; exit 5; }
import json, sys
def flat(x, p=""):
    if isinstance(x, dict):
        return {k: v for n, c in x.items() for k, v in flat(c, f"{p}.{n}" if p else n).items()}
    if isinstance(x, list):
        return {k: v for i, c in enumerate(x) for k, v in flat(c, f"{p}.{i}").items()}
    return {p: x}
a, b = (flat(json.load(open(f))) for f in sys.argv[1:3])
changed = sorted(k for k in a.keys() | b.keys() if a.get(k) != b.get(k))
print("changed", changed)
sys.exit(0 if changed == sorted(sys.argv[3:]) else 5)
EOF
  cp -p "$F" "$B/compose.test.yml.before-$step"; mv "$F.new" "$F"; date -u +%FT%TZ
  compose "$F" up -d --no-deps "$step" 2>&1 | tail -3; }
backup_dir() { local f listed; for f in compose.test.yml challenge.db server_keys.db SHA256SUMS; do
    [ -f "$1/$f" ] || { echo "not a backup dir: $1 (no $f)"; exit 4; }; done
  # --strict fails on a malformed line instead of skipping it. A SHA256SUMS cut short still verifies
  # the files it lists, so every file used later must be listed too ("HASH  NAME" or "HASH *NAME").
  # Files added afterwards (config-*.json, compose.test.yml.before-*, restore-db-aside-*) are never used.
  (cd "$1" && sha256sum -c --strict --quiet SHA256SUMS) || { echo "backup $1 fails SHA256SUMS"; exit 4; }
  listed=$(sed -E 's/^[0-9a-fA-F]{64} [ *]//' "$1/SHA256SUMS")
  for f in compose.test.yml challenge.db server_keys.db compose.relay.yml relay.json metagraph.json \
      inspect-before.json; do
    if [ -e "$1/$f" ] && ! grep -qxF -- "$f" <<< "$listed"; then
      echo "backup $1: $f is not listed in SHA256SUMS"; exit 4; fi; done; }
case $cmd in
backup)
  TS=${2:?TS}; B=/root/backups/hypertrain-test-s7-$TS
  [ ! -e "$B" ] || { echo "exists: $B"; exit 4; }
  install -d -m 0700 "$B"; cp -p "$F" "$T/compose.relay.yml" "$B/"
  cp -p /data/relay/config/relay.json /data/hypertrain-test/config/metagraph.json "$B/"
  python3 -I - "$STATE" "$B" <<'EOF' || { echo "DB backup not ok"; exit 6; }
import sqlite3, sys
state, out = sys.argv[1:]
ok = True
for name in ("challenge.db", "server_keys.db"):
    src = sqlite3.connect(f"file:{state}/{name}?mode=ro", uri=True)
    dst = sqlite3.connect(f"{out}/{name}")
    src.backup(dst)
    result = dst.execute("pragma integrity_check").fetchone()[0]
    print(name, result)
    ok = ok and result == "ok"
    dst.close(); src.close()
sys.exit(0 if ok else 6)
EOF
  chmod 0600 "$B"/*.db
  docker inspect "$C" hypertrain-test-beacon hypertrain-test-master-stub hypertrain-test-relay \
    | python3 -I -c '
import json,sys
cs=json.load(sys.stdin)
for c in cs: c["Config"]["Env"]=sorted(e.split("=",1)[0] for e in c["Config"].get("Env") or [])
json.dump(cs,sys.stdout,indent=1)' > "$B/inspect-before.json"
  (cd "$B" && sha256sum compose.test.yml compose.relay.yml challenge.db server_keys.db \
    relay.json metagraph.json inspect-before.json > SHA256SUMS && cat SHA256SUMS); echo "backup $B";;
swap)
  NEW=${2:?NEW_REF}; B=${3:?BACKUP}; backup_dir "$B"; gate
  [[ $NEW =~ ^ghcr\.io/cortexlm/cortex/hypertrain@sha256:[0-9a-f]{64}$ ]] || { echo "bad ref"; exit 4; }
  [ "$(docker image inspect "$NEW" --format '{{.Id}}')" = "${NEW#*@}" ] || { echo "image not pulled"; exit 4; }
  grep -c '^    container_name: hypertrain-test-challenge$' "$F" | grep -qx 1 || { echo "anchor"; exit 4; }
  umask 077; sed "/^    container_name: hypertrain-test-challenge\$/a\\    image: $NEW" "$F" > "$F.new"
  apply challenge "$B" services.challenge.image
  images "$F"; live "${NEW#*@}"; sleep 6; date -u +%FT%TZ; check;;
beacon)
  B=${2:?BACKUP}; backup_dir "$B"; gate
  umask 077; sed -e 's|--interval, "3",$|--interval, "0.2",|' \
    -e 's|--catch-up, "20", --max-failures|--catch-up, "3", --max-failures|' "$F" > "$F.new"
  apply beacon "$B" services.beacon.entrypoint.9 services.beacon.entrypoint.11
  sleep 20; docker logs --since 25s hypertrain-test-beacon 2>&1 | tail -4 | cut -c1-160
  curl -s -m 10 "$API/v1/beacon/latest" | python3 -I -c '
import json,sys,time
r=json.load(sys.stdin)["round"]; print("lag", (int(time.time())-1692803367)//3+1-r)';;
paper)
  B=${2:?BACKUP}; backup_dir "$B"; backup_names "$B"; images "$B/compose.test.yml"
  echo "live: $(docker inspect "$C" --format '{{.Config.Image}}')"
  echo "would run: cp -p $B/compose.test.yml $F && docker compose -f $F up -d --no-deps challenge beacon";;
rollback)
  B=${2:?BACKUP}; backup_dir "$B"; backup_names "$B"; date -u +%FT%TZ
  OLD=$(python3 -I -c 'import json,sys;print(next(c["Image"] for c in json.load(open(sys.argv[1]))
    if c["Name"]=="/hypertrain-test-challenge"))' "$B/inspect-before.json")
  cp -p "$B/compose.test.yml" "$F"; compose "$F" up -d --no-deps challenge beacon 2>&1 | tail -3
  live "$OLD"
  for pair in "relay.json:/data/relay/config/relay.json:hypertrain-test-relay" \
      "metagraph.json:/data/hypertrain-test/config/metagraph.json:hypertrain-test-master-stub"; do
    IFS=: read -r name path container <<< "$pair"
    if [ -f "$B/$name" ] && ! cmp -s "$B/$name" "$path"; then
      cat "$B/$name" > "$path"; docker restart "$container"; echo "restored $path"; fi
  done
  sleep 6; check;;
restore-db)
  B=${2:?BACKUP}; backup_dir "$B"; A=$B/restore-db-aside-$(date -u +%Y%m%dT%H%M%SZ)
  install -d -m 0700 "$A"; cp -p "$STATE"/challenge.db* "$STATE"/server_keys.db* "$A/"
  docker stop "$C"
  for n in challenge.db server_keys.db; do
    rm -f "$STATE/$n-wal" "$STATE/$n-shm"; install -m 0644 -o 65532 -g 65532 "$B/$n" "$STATE/$n"; done
  docker start "$C"; sleep 6; check;;
executor)
  B=${2:?BACKUP}; P=${3:?CONTAINER_CONFIG_PATH}; backup_dir "$B"; gate
  [[ $P =~ ^/data/[A-Za-z0-9._/-]+\.json$ ]] || { echo "PATH must be /data/....json in the container"; exit 4; }
  O=$T/compose.executor.yml; umask 077
  printf 'services:\n  challenge:\n    environment:\n      HYPERTRAIN_CUDA_EXECUTOR: %s\n' "$P" > "$O.new"
  compose "$F" config --format json > "$B/config-before-executor.json"
  (cd "$T" && docker compose -f "$F" -f "$O.new" config --format json) > "$B/config-after-executor.json"
  python3 -I - "$B/config-before-executor.json" "$B/config-after-executor.json" \
      services.challenge.environment.HYPERTRAIN_CUDA_EXECUTOR <<'EOF' \
    || { rm -f "$O.new"; echo "unexpected compose change"; exit 5; }
import json, sys
def flat(x, p=""):
    if isinstance(x, dict):
        return {k: v for n, c in x.items() for k, v in flat(c, f"{p}.{n}" if p else n).items()}
    if isinstance(x, list):
        return {k: v for i, c in enumerate(x) for k, v in flat(c, f"{p}.{i}").items()}
    return {p: x}
a, b = (flat(json.load(open(f))) for f in sys.argv[1:3])
changed = sorted(k for k in a.keys() | b.keys() if a.get(k) != b.get(k))
print("changed", changed)
sys.exit(0 if changed == sorted(sys.argv[3:]) else 5)
EOF
  mv "$O.new" "$O"; date -u +%FT%TZ
  (cd "$T" && docker compose -f "$F" -f "$O" up -d --no-deps challenge 2>&1 | tail -3)
  live "$(docker inspect "$C" --format '{{.Image}}')"; sleep 6; check;;
executor-off)
  B=${2:?BACKUP}; backup_dir "$B"; date -u +%FT%TZ
  rm -f "$T/compose.executor.yml"; compose "$F" up -d --no-deps challenge 2>&1 | tail -3
  live "$(docker inspect "$C" --format '{{.Image}}')"; sleep 6; check;;
*) echo "unknown $cmd"; exit 2;;
esac
