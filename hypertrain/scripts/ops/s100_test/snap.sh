#!/usr/bin/env bash
# Read-only snapshot of the s100 hypertrain TEST stack, plus the production liveness signals
# that must not move. Prints no secret: env is reduced to names, tokens are never read.
# Run on s100 over stdin, nothing is written there: ssh root@s100 'bash -s -- LABEL' < snap.sh
# The production section comes first, and a failing TEST probe prints FAILED instead of
# stopping the script: after a bad change both views are needed.
set -uo pipefail
LABEL=${1:?label}
T=/root/backups/hypertrain-test; API=http://127.0.0.1:18996
MASTER=http://10.177.245.39:8080
echo "== snap $LABEL $(date -u +%FT%TZ)"
echo "== prod (must not move)"
docker ps --format '{{.Names}}\t{{.Image}}\t{{.Status}}' \
  | grep -E '^(cortex-challenge-hypertrain|hypertrain-auditor-go-[01]|hypertrain-beacon|cortex-master-)' \
  | sort
docker inspect cortex-challenge-hypertrain --format '{{.Image}} {{.State.StartedAt}} restarts={{.RestartCount}}'
curl -s -m 10 -o /dev/null -w 'master readyz %{http_code} ' "$MASTER/readyz"; curl -s -m 10 "$MASTER/readyz"; echo
curl -s -m 10 "$MASTER/v1/weights/latest" | python3 -I -c '
import json,sys,datetime
d=json.load(sys.stdin); c=datetime.datetime.fromisoformat(d["computed_at"].replace("Z","+00:00"))
age=(datetime.datetime.now(datetime.timezone.utc)-c).total_seconds()
print("seal epoch",d["epoch"],"sealed",d.get("sealed"),"computed_at",d["computed_at"],"age_s",int(age))' \
  || echo "seal FAILED"
echo "== test"
docker ps -a --format '{{.Names}}\t{{.Image}}\t{{.Status}}' | grep -E '^hypertrain-test-' | sort \
  || echo "no hypertrain-test containers"
for n in hypertrain-test-challenge hypertrain-test-beacon hypertrain-test-master-stub \
  hypertrain-test-relay hypertrain-test-auditor-0 hypertrain-test-auditor-1; do
  docker inspect "$n" 2>/dev/null | python3 -I -c '
import json,sys
c=json.load(sys.stdin)[0]
print(c["Name"][1:], c["Image"], c["Config"]["Image"], c["State"]["Status"],
      c["State"]["StartedAt"], "restarts=%d" % c["RestartCount"])' || echo "$n absent"
done
for p in version health public/v1/runs v1/beacon/latest; do
  printf '%s ' "$p"; curl -s -m 10 -w ' %{http_code}\n' "$API/$p" | cut -c1-300 || echo FAILED
done
printf 'public runs sha '; curl -s -m 10 "$API/public/v1/runs" | sha256sum | cut -c1-64
printf 'public /test-master/version '
curl -s -m 10 -o /dev/null -w '%{http_code}\n' https://relay.joinbase.ai/test-master/version
for p in livez readyz; do
  printf 'relay %s ' "$p"; curl -s -m 10 -w ' %{http_code}\n' "http://127.0.0.1:18995/$p" | cut -c1-200
done
sha256sum "$T/compose.test.yml" "$T/compose.relay.yml" /data/relay/config/relay.json \
  /data/hypertrain-test/config/metagraph.json | sed 's|  | |'
python3 -I - <<'EOF' || echo "test db FAILED"
import sqlite3
db = sqlite3.connect("file:/data/hypertrain-test/state/challenge.db?mode=ro", uri=True)
names = [r[0] for r in db.execute("select name from sqlite_master where type='table'")]
counts = {n: db.execute(f'select count(*) from "{n}"').fetchone()[0]
          for n in ("runs", "records_v2", "audit_leases_v2") if n in names}
print("test db", counts, "tables", len(names))
EOF
