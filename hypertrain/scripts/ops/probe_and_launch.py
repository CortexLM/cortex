#!/usr/bin/env python3
# ruff: noqa: E501, S310, S603, S607  (verbatim controller tool: fixed Vast API URL, argv lists)
"""Find two healthy Vast 8x RTX 5090 hosts (rent, wait for boot, delete), then prepare and launch.

Vast hosts often fail container start with "failed to inject CDI devices" (GPUs not attachable);
the launcher's boot() then kills the whole admission. Each candidate is rented with the exact
lifecycle image, watched until running+ssh or failure, and always deleted (absence verified)
before the real admission rents it again. A booted probe must also prove real bandwidth (16-stream
ssh egress to this controller, ranged HF ingress): advertised inet_up does not predict it.
Log lines: PROBE, PAIR, LAUNCHED, GIVEUP.
ponytail: an ask can vanish between probe and admission; the launcher then fails at admit and
this script must be re-run.
"""

import concurrent.futures as cf
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

from hf_seed import URL as HF_URL

D = "/data/role-dispatch"
API = "https://console.vast.ai/api/v0"
KEY = open(f"{D}/keys/vast.key").read().strip()
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
LAUNCHER = f"{REPO}/scripts/launch_cuda_run.sh"
HT_TREE = os.environ.get("HT_TREE") or sys.exit("HT_TREE must name the controller tree")
SRC = os.environ.get("SRC", "v1")
OUT, JOB, SRC_PREP = f"{D}/manifest/out-{SRC}", f"{D}/manifest/job-{SRC}", f"{D}/prep-{SRC}"
# Frozen controller tree (prep-v3 = direct-ssh launcher); context stays the signed run's.
FREEZE_PREP = os.environ.get("FREEZE_PREP", SRC_PREP)
PROFILE = json.load(open(f"{OUT}/profile.json"))
ALLOW = set(PROFILE["runtime"]["driver_allowlist"])
ADM = PROFILE["admission"]
IMAGE = json.load(open(f"{D}/prep-v1/base/private/lifecycle-config.json"))["image"]
OFFER_KEYS = (
    "cpu_cores_effective",
    "cpu_ram",
    "disk_space",
    "dph_total",
    "driver_version",
    "gpu_name",
    "id",
    "inet_down",
    "inet_down_cost",
    "inet_up",
    "inet_up_cost",
    "machine_id",
    "num_gpus",
    "reliability2",
    "storage_cost",
)
MIN_MBPS = 800  # network_gpu_prepare.py MIN_INET_MBPS
BOOT_S, CYCLE_S, MAX_CYCLES = 300, 120, int(os.environ.get("MAX_CYCLES", "24"))
# Measured gates: 16 x 256 MiB ssh streams host->controller for <=30 s, one ranged HF curl.
EGRESS_MIN, INGRESS_MIN, BW_S, STREAMS = 700, 400, 30, 16
KEYDIR = tempfile.mkdtemp(prefix="ht-probe-")
PROBE_KEY = f"{KEYDIR}/id_ed25519"


def us_first(o):
    return (not str(o.get("geolocation") or "").endswith(", US"), o["dph_total"])


def log(*a):
    print(time.strftime("%FT%TZ", time.gmtime()), *a, flush=True)


def call(method, path, body=None, base=API):
    for attempt in range(8):
        req = urllib.request.Request(
            base + path,
            method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": "Bearer " + KEY, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            if e.code != 429 and e.code < 500 or attempt == 7:
                raise
            time.sleep(min(60, 3 * 2**attempt))


def owned_machines():
    rows = call("GET", "/instances/?owner=me", base="https://console.vast.ai/api/v1")
    return {i["machine_id"] for i in rows.get("instances", [])}


def offers(machines=None):
    q = {
        "gpu_name": {"eq": "RTX 5090"},
        "num_gpus": {"eq": 8},
        "rentable": {"eq": True},
        "disk_space": {"gte": ADM["min_disk_gib"]},
        "direct_port_count": {"gte": 1},
        "type": "on-demand",
        "limit": 200,
        "order": [["dph_total", "asc"]],
    }
    if machines:
        q["machine_id"] = {"in": sorted(machines)}
    return [
        o
        for o in call("POST", "/bundles/", q)["offers"]
        if o["driver_version"] in ALLOW
        and o["dph_total"] <= float(ADM["max_host_hourly_usd"])
        and o["cpu_cores_effective"] >= ADM["min_cpu_cores"]
        and o["cpu_ram"] / 1024 >= ADM["min_host_ram_gib"]
        and o["inet_up"] >= MIN_MBPS
        and o["inet_down"] >= MIN_MBPS
    ]


def bandwidth(ip, port):
    """(egress, ingress) Mbit/s measured on a running probe host, or None when ssh never answers."""
    ssh = [
        "ssh",
        "-i",
        PROBE_KEY,
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "Compression=no",
        "-p",
        str(port),
        f"root@{ip}",
    ]
    t0 = time.time()
    while subprocess.run([*ssh, "true"], capture_output=True, timeout=60).returncode:
        if time.time() - t0 > 120:  # key propagation + sshd start
            return None
        time.sleep(10)
    got = [0] * STREAMS

    def stream(i):
        p = subprocess.Popen(
            [*ssh, f"head -c {256 << 20} /dev/zero"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        end = time.time() + BW_S
        try:
            while time.time() < end and (b := p.stdout.read1(1 << 20)):
                got[i] += len(b)
        finally:
            p.kill()
            p.wait()

    t0 = time.time()
    threads = [threading.Thread(target=stream, args=(i,)) for i in range(STREAMS)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    egress = sum(got) * 8 / (time.time() - t0) / 1e6
    r = subprocess.run(
        [
            *ssh,
            f"curl -sL -o /dev/null -r 0-{512 << 20} --max-time {BW_S} "
            f"-w '%{{size_download}} %{{time_total}}' {HF_URL}",
        ],
        capture_output=True,
        text=True,
        timeout=BW_S + 60,
    )
    try:
        size, secs = map(float, r.stdout.split()[-2:])
        ingress = size * 8 / secs / 1e6
    except ValueError:
        ingress = 0.0
    return egress, ingress


def probe(o):
    mid = o["machine_id"]
    try:
        put = call(
            "PUT",
            f"/asks/{o['id']}/",
            {
                "image": IMAGE,
                "disk": ADM["min_disk_gib"],
                "runtype": "ssh_direc ssh_proxy",
                "target_state": "running",
                "cancel_unavail": True,
                "onstart": "true",
                "label": f"ht-probe-{mid}",
            },
        )
    except Exception as e:  # ask vanished / HTTP error: not healthy, nothing rented
        return mid, f"rent_failed {e}"
    iid = put.get("new_contract")
    if not iid:
        return mid, f"rent_failed {str(put)[:120]}"
    verdict, t0 = "boot_timeout", time.time()
    try:
        while time.time() - t0 < BOOT_S:
            st = call("GET", f"/instances/{iid}/?owner=me").get("instances") or {}
            msg = st.get("status_msg") or ""
            direct = ((st.get("ports") or {}).get("22/tcp") or [{}])[0].get("HostPort")
            if st.get("actual_status") == "running" and direct:
                up = f"{int(time.time() - t0)}s direct={st.get('public_ipaddr')}:{direct}"
                call(
                    "POST",
                    f"/instances/{iid}/ssh/",
                    {"ssh_key": open(PROBE_KEY + ".pub").read().strip()},
                )
                try:
                    bw = bandwidth(st.get("public_ipaddr"), direct)
                except subprocess.TimeoutExpired:
                    bw = None
                if bw is None:
                    verdict = f"slow ssh_unreachable {up}"
                else:
                    ok = bw[0] >= EGRESS_MIN and bw[1] >= INGRESS_MIN
                    verdict = (
                        f"{'HEALTHY' if ok else 'slow'} {up} egress={bw[0]:.0f}Mbit/s "
                        f"ingress={bw[1]:.0f}Mbit/s"
                    )
                break
            if (
                "CDI" in msg
                or "Error response" in msg
                or st.get("actual_status") in ("exited", "offline")
                or st.get("cur_state") == "stopped"
            ):
                verdict = "failed " + msg.strip().splitlines()[-1][:120] if msg else "failed"
                break
            time.sleep(10)
    finally:
        for _ in range(6):
            try:
                call("DELETE", f"/instances/{iid}/")
                if not call("GET", f"/instances/{iid}/?owner=me").get("instances"):
                    break
            except Exception as e:
                log("PROBE delete retry", mid, iid, e)
            time.sleep(10)
        else:
            log("PROBE DELETE_UNCONFIRMED", mid, iid)
    return mid, verdict


def run(cmd, **kw):
    return subprocess.run(["bash", "-c", cmd], capture_output=True, text=True, **kw)


def launch(pair, tag):
    prep, now = f"{D}/prep-{tag}", int(time.time())
    for _ in range(24):  # a just-deleted probe takes a while to be listed as rentable again
        fresh = {o["machine_id"]: o for o in offers(pair)}
        if set(fresh) == set(pair):
            break
        time.sleep(15)
    if set(fresh) != set(pair):
        return f"asks gone {sorted(set(pair) - set(fresh))}"
    sel = [{**{k: fresh[m][k] for k in OFFER_KEYS}, "received_unix": now} for m in pair]
    sel_path = f"{D}/prep-r5/selected-pair-{tag}-offers.json"
    with open(sel_path, "w") as f:
        json.dump(sel, f)
    os.chown(sel_path, 65532, 65532)
    os.chmod(sel_path, 0o600)
    env = (
        f"export PYTHONHOME={D}/runtime/local CUDA_VISIBLE_DEVICES= HOME={D} "
        f"PYTHONPATH={HT_TREE}/src:{HT_TREE}/scripts:"
        f"{HT_TREE}:/data/pydeps:{D}/runtime/venv/lib/python3.12/site-packages;"
    )
    as_rt = "setpriv --reuid=65532 --regid=65532 --clear-groups"
    py = f"{D}/runtime/venv/bin/python"
    r = run(f"""set -e; {env}
install -m 0755 {REPO}/scripts/network_gpu_prepare.py /tmp/ngp-vast.py
install -m 0755 {REPO}/scripts/ops/prepare_runtime.py /tmp/prepare-runtime-vast.py
install -d -o 65532 -g 65532 -m 0700 {prep}; cp -a {FREEZE_PREP}/freeze {SRC_PREP}/context {prep}/; cd {D}
{as_rt} {py} /tmp/ngp-vast.py --freeze {prep}/freeze/freeze.json --output {prep}/base \
  --cpu-genesis-unix {now} --qualification-base {OUT} {JOB} {sel_path} \
  {D}/custody/index.manifest.json {D}/custody/amd64.manifest.json > /dev/null
{as_rt} {py} /tmp/prepare-runtime-vast.py evidence --base {prep}/base/private/review-base.json \
  --destination {prep}/evidence > /dev/null
HT_TREE= TAG={tag} PREP={prep} RUN={D}/run-{tag} OUT={OUT} JOB={JOB} \
  timeout 250 {LAUNCHER} --dry-run""")
    if "DRY_RUN_DONE" not in r.stdout:
        return f"prepare/dry-run failed: {(r.stdout + r.stderr)[-600:]}"
    subprocess.Popen(
        [
            "setsid",
            "bash",
            "-c",
            f"HT_TREE= TAG={tag} PREP={prep} RUN={D}/run-{tag} OUT={OUT} JOB={JOB} "
            f"exec {LAUNCHER} --yes-spend > {D}/launch-{tag}.log 2>&1 < /dev/null",
        ],
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return None


def main():
    tag = sys.argv[1]
    bad: dict[int, int] = {}
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", PROBE_KEY], check=True)
    for cycle in range(MAX_CYCLES):
        busy = owned_machines()
        cands = [
            o for o in offers() if o["machine_id"] not in busy and bad.get(o["machine_id"], 0) < 2
        ]
        log(
            "CYCLE",
            cycle,
            "candidates",
            [(o["machine_id"], o["driver_version"], round(o["dph_total"], 2)) for o in cands],
        )
        # US hosts first (controller link), then price; one wave, 2 s apart (under Vast's 429 rate).
        cands.sort(key=us_first)
        with cf.ThreadPoolExecutor(len(cands[:10]) or 1) as ex:
            futures = []
            for o in cands[:10]:
                futures.append(ex.submit(probe, o))
                time.sleep(2)
            results = [f.result() for f in futures]
        healthy = []
        for mid, verdict in results:
            log("PROBE", mid, verdict)
            if verdict.startswith("HEALTHY"):
                healthy.append(mid)
            elif verdict.startswith("failed"):
                bad[mid] = bad.get(mid, 0) + 1
        if len(healthy) >= 2:
            rank = {o["machine_id"]: us_first(o) for o in cands}
            pair = sorted(healthy, key=rank.get)[:2]
            log("PAIR", pair)
            err = launch(pair, tag)
            if err is None:
                log("LAUNCHED", tag, f"log={D}/launch-{tag}.log")
                return 0
            log("LAUNCH_SKIPPED", err)
        time.sleep(CYCLE_S)
    log("GIVEUP no healthy compliant 8x5090 pair after", MAX_CYCLES, "cycles")
    return 1


if __name__ == "__main__":
    sys.exit(main())
