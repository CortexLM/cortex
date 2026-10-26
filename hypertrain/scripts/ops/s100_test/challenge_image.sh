#!/usr/bin/env bash
# Swap the image of the s100 TEST challenge (compose service `challenge` only) and roll it back.
# Never touches the production challenge, the master, the beacon, the relay or the master stub.
# Run on s100 over stdin: ssh root@s100 'bash -s -- CMD ARGS' < challenge_image.sh
#   backup TS            snapshot compose files, both SQLite DBs and inspect JSON (env names only)
#   swap NEW_REF BACKUP  gate, pin NEW_REF on the challenge service, up -d --no-deps challenge
#   paper BACKUP         dry rollback: the backed-up compose file must parse and resolve the old image
#   rollback BACKUP      restore the compose file and recreate the challenge on the old image
#   restore-db BACKUP    stop the challenge, put the backed-up DBs back, start it (only if needed)
set -euo pipefail
T=/root/backups/hypertrain-test; F=$T/compose.test.yml; C=hypertrain-test-challenge
STATE=/data/hypertrain-test/state; API=http://127.0.0.1:18996; MASTER=http://10.177.245.39:8080
cmd=${1:?backup|swap|paper|rollback|restore-db}
compose() { (cd "$T" && docker compose -f "$1" "${@:2}"); }
images() { # service -> image as compose resolves it
  compose "$1" config --format json | python3 -I -c '
import json,sys
for n,s in sorted(json.load(sys.stdin)["services"].items()): print(n, s["image"])'; }
gate() { # readyz 200 and a sealed seal younger than 30 min: first half of the 72-min window
  curl -sf -m 10 -o /dev/null "$MASTER/readyz" || { echo "GATE readyz not 200"; exit 2; }
  curl -s -m 10 "$MASTER/v1/weights/latest" | python3 -I -c '
import json,sys,datetime
d=json.load(sys.stdin); c=datetime.datetime.fromisoformat(d["computed_at"].replace("Z","+00:00"))
age=(datetime.datetime.now(datetime.timezone.utc)-c).total_seconds()
print("gate seal epoch",d["epoch"],"sealed",d.get("sealed"),"computed_at",d["computed_at"],"age_s",int(age))
sys.exit(0 if d.get("sealed") is True and 0<=age<1800 else 3)' \
    || { echo "GATE seal not sealed or older than 30 min"; exit 3; }; }
check() { # what an operator reads after any change
  docker inspect "$C" --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}} restarts={{.RestartCount}}'
  for p in version health public/v1/runs; do
    printf '%s ' "$p"; curl -s -m 10 -w ' %{http_code}\n' "$API/$p" | cut -c1-200; done
  printf 'public runs sha '; curl -s -m 10 "$API/public/v1/runs" | sha256sum | cut -c1-64; }
backup_dir() { [ -d "$1" ] && [ -f "$1/compose.test.yml" ] && [ -f "$1/challenge.db" ] \
  || { echo "not a backup dir: $1"; exit 4; }; }
case $cmd in
backup)
  TS=${2:?TS}; B=/root/backups/hypertrain-test-s7-$TS
  [ ! -e "$B" ] || { echo "exists: $B"; exit 4; }
  install -d -m 0700 "$B"; cp -p "$F" "$T/compose.relay.yml" "$B/"
  python3 -I - "$STATE" "$B" <<'EOF'
import sqlite3, sys
state, out = sys.argv[1:]
for name in ("challenge.db", "server_keys.db"):
    src = sqlite3.connect(f"file:{state}/{name}?mode=ro", uri=True)
    dst = sqlite3.connect(f"{out}/{name}")
    src.backup(dst)
    print(name, dst.execute("pragma integrity_check").fetchone()[0])
    dst.close(); src.close()
EOF
  chmod 0600 "$B"/*.db
  docker inspect "$C" hypertrain-test-beacon hypertrain-test-master-stub hypertrain-test-relay \
    | python3 -I -c '
import json,sys
cs=json.load(sys.stdin)
for c in cs: c["Config"]["Env"]=sorted(e.split("=",1)[0] for e in c["Config"].get("Env") or [])
json.dump(cs,sys.stdout,indent=1)' > "$B/inspect-before.json"
  (cd "$B" && sha256sum compose.test.yml compose.relay.yml challenge.db server_keys.db \
    inspect-before.json > SHA256SUMS && cat SHA256SUMS); echo "backup $B";;
swap)
  NEW=${2:?NEW_REF}; B=${3:?BACKUP}; backup_dir "$B"; gate
  [[ $NEW =~ ^ghcr\.io/cortexlm/cortex/hypertrain@sha256:[0-9a-f]{64}$ ]] || { echo "bad ref"; exit 4; }
  [ "$(docker image inspect "$NEW" --format '{{.Id}}')" = "${NEW#*@}" ] || { echo "image not pulled"; exit 4; }
  cmp -s "$F" "$B/compose.test.yml" || { echo "compose changed since backup"; exit 4; }
  grep -c '^    container_name: hypertrain-test-challenge$' "$F" | grep -qx 1 || { echo "anchor"; exit 4; }
  umask 077; sed "/^    container_name: hypertrain-test-challenge\$/a\\    image: $NEW" "$F" > "$F.new"
  diff "$F" "$F.new" || true
  images "$F.new" > "$B/images-after.txt"; images "$F" > "$B/images-before.txt"
  diff "$B/images-before.txt" "$B/images-after.txt" || true
  # exactly one resolved image may differ: the challenge, and only to NEW
  python3 -I - "$B/images-before.txt" "$B/images-after.txt" "$NEW" <<'EOF'
import sys
a, b, new = sys.argv[1:]
before = dict(l.split() for l in open(a)); after = dict(l.split() for l in open(b))
diff = {k for k in before if before[k] != after.get(k)} | (after.keys() - before.keys())
sys.exit(0 if diff == {"challenge"} and after["challenge"] == new else 5)
EOF
  mv "$F.new" "$F"; date -u +%FT%TZ
  compose "$F" up -d --no-deps challenge 2>&1 | tail -3
  for i in $(seq 1 30); do curl -sf -m 5 -o /dev/null "$API/version" && break; sleep 1; done
  sleep 6; date -u +%FT%TZ; check;;
paper)
  B=${2:?BACKUP}; backup_dir "$B"; images "$B/compose.test.yml"
  echo "live: $(docker inspect "$C" --format '{{.Config.Image}}')"
  echo "would run: cp -p $B/compose.test.yml $F && docker compose -f $F up -d --no-deps challenge";;
rollback)
  B=${2:?BACKUP}; backup_dir "$B"; date -u +%FT%TZ
  cp -p "$B/compose.test.yml" "$F"; compose "$F" up -d --no-deps challenge 2>&1 | tail -3
  for i in $(seq 1 30); do curl -sf -m 5 -o /dev/null "$API/version" && break; sleep 1; done
  sleep 6; check;;
restore-db)
  B=${2:?BACKUP}; backup_dir "$B"; docker stop "$C"
  for n in challenge.db server_keys.db; do
    rm -f "$STATE/$n-wal" "$STATE/$n-shm"; install -m 0644 -o 65532 -g 65532 "$B/$n" "$STATE/$n"; done
  docker start "$C"; sleep 6; check;;
*) echo "unknown $cmd"; exit 2;;
esac
