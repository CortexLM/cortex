"""Drive N disposable CPU miners against the s100 TEST challenge with the real miner CLI only.

Every miner action is one `hypertrain-miner` process (join-v2, escrow-lock, probe-v2, proof-v2,
status-v2, run-v2) against API (the public https://relay.joinbase.ai/test-master); rounds,
audits and finalize stay with the operator (live_operator.py on the challenge host).
Each stage is timed into DIR/stages.tsv; seeds stay 0600 in DIR/m<i>/ and are never printed.

  setup DIR API RUN_ID OWNER_SS58 IMAGE_DIGEST SEEDS_DIR ORIGINS_JSON N
        SEEDS_DIR/miner<i>-hot.seed, miner<i>-cold.seed; ORIGINS_JSON from publish_run.py genesis
  admit DIR PLAN       PLAN "1:12,2:12,3:12,0:4": miner:target clean trials, in order
  round DIR W I [I..]  run-v2 --round W for the listed miners, in parallel
A trial is retried only on the transient refusals the challenge documents (503, SEED_NOT_READY,
TRIAL_CAPACITY) and on the one 404 the miner reports once its own retries of a trial input are
spent (RELAY-02: a read miss on an intact object); any other failure stops the plan, since a
missed trial strands the identity (409 CHALLENGE_EXPIRED): probe-v2 then waits 10 minutes for the
operator's reseed (live_operator.py reseed, docs/s100-test-stack.md section 5) and exits with
status 3, which stops the plan too.

ATEN_CPU_CAPABILITY must name the challenge host's ATen CPU capability (avx2 for s100, a Zen 3).
Trials and audits score only on bitwise equality with the challenge's own CPU replay, and the
AVX-512 kernels of a newer miner host give data-dependent last-bit differences in the optimizer
state (2026-10-26: trial 6 of 6 SUSPENDED PENDING_REPLAY_MISMATCH; the same job matched under
ATEN_CPU_CAPABILITY=avx2). admit and round refuse to start without it.

HT_MINER_DEVICE=cuda drives a GPU miner for a CUDA run instead: run this script with the GPU image's
python (`docker run --gpus device=N -e HT_IMAGE_DIGEST=<manifest image digest> -e
HT_MINER_DEVICE=cuda`, docs/s100-test-stack.md section 6.5). The ATen pin is then not needed
(the reference runs on a GPU host), HT_IMAGE_DIGEST is, a CLI stage that uses the GPU first waits
for the GPU to be idle, and a 500 from the challenge (its remote reference failed) is retried.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import httpx

HM = str(Path(sys.executable).with_name("hypertrain-miner"))
# Refusals the challenge or its front documents as temporary, matched on the CLI's final error
# line (stderr) or its JSON error (stdout) only: the INFO log above them holds hex ids.
TRANSIENT = re.compile(
    r"ChallengeHTTPError: 50[234]\b|\"status\": 50[234]\b|SEED_NOT_READY|TRIAL_CAPACITY"
    r"|beacon infrastructure paused|authenticated assignment not ready"
    r"|ChallengeHTTPError: 404: trial input object unavailable"
    r"|ConnectError|ConnectTimeout|ReadTimeout|WriteTimeout|RemoteProtocolError|ReadError"
)
DEVICE = os.environ.get("HT_MINER_DEVICE", "cpu")
if DEVICE == "cuda":
    TRANSIENT = re.compile(TRANSIENT.pattern + r"|ChallengeHTTPError: 500\b")
GPU_STAGES = ("probe-v2", "run-v2")
ENV = {**os.environ, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1", "PYTHONHASHSEED": "0"}
LOCK = threading.Lock()


def wait_gpu_idle(limit_mib: int = 2048, patience: float = 240) -> None:
    """Nothing of ours holds the GPU between two CLI processes, so memory in use is foreign: the
    Lium validator's matmul holds ~265 GiB for minutes and the CUDA worker would die with OOM.
    240 s: the first challenge is due 100 rounds (5 min) after the join."""
    smi = shutil.which("nvidia-smi")
    if smi is None:
        raise SystemExit("nvidia-smi unavailable")
    start = time.monotonic()
    while True:
        query = subprocess.run(  # noqa: S603 - PATH-resolved nvidia-smi, fixed query
            [smi, "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            check=True,
            timeout=20,
        )
        used = max(int(x) for x in query.stdout.split())
        if used <= limit_mib:
            return
        if time.monotonic() - start > patience:
            raise SystemExit(f"GPU still busy after {patience:.0f}s: {used} MiB in use")
        time.sleep(10)


def log(root: Path, row: dict[str, object]) -> None:
    row = {"utc": time.strftime("%H:%M:%SZ", time.gmtime()), **row}
    with LOCK, open(root / "stages.tsv", "a") as handle:
        handle.write(json.dumps(row, sort_keys=True) + "\n")
    print(json.dumps(row, sort_keys=True), flush=True)


def cli(root: Path, i: int, name: str, args: list[str], **extra: object) -> tuple[int, str, str]:
    m = root / f"m{i}"
    if DEVICE == "cuda" and name in GPU_STAGES:
        wait_gpu_idle()
    t0 = time.monotonic()
    p = subprocess.run(  # noqa: S603 - fixed miner CLI argv, no shell
        [HM, *args], cwd=m, env=ENV, capture_output=True, text=True, timeout=1800
    )
    secs = round(time.monotonic() - t0, 2)
    with open(m / f"{name}.log", "a") as handle:
        handle.write(f"## {time.strftime('%H:%M:%SZ', time.gmtime())} rc={p.returncode}\n")
        handle.write(p.stderr[-4000:] + "\n")
    log(
        root,
        {
            "miner": i,
            "stage": name,
            "rc": p.returncode,
            "s": secs,
            "out": p.stdout.strip()[-300:],
            **extra,
        },
    )
    return p.returncode, p.stdout, p.stderr


def transient(out: str, err: str) -> bool:
    last = [line for line in err.strip().splitlines() if line.strip()][-1:]
    return bool(TRANSIENT.search(out.strip()[-500:] + "\n" + "".join(last)))


def now_round(api: str) -> int:
    return int(httpx.get(api + "/v1/beacon/latest", timeout=30).json()["round"])


def setup(argv: list[str]) -> int:
    root, api, run_id, owner, image, seeds, origins, n = argv
    root = Path(root)
    root.mkdir(mode=0o700, exist_ok=True)
    origin_ids = json.loads(Path(origins).read_text())
    for i in range(int(n)):
        m = root / f"m{i}"
        m.mkdir(mode=0o700)
        (m / "work").mkdir(mode=0o700)
        for role in ("hot", "cold"):
            shutil.copyfile(Path(seeds) / f"miner{i}-{role}.seed", m / f"{role}.seed")
            os.chmod(m / f"{role}.seed", 0o600)
        conf = {
            "api": api,
            "keyfile": "hot.seed",
            "workdir": "work",
            "state_source": f"{api}/v2/runs/{run_id}/objects",
            "image_digest": image,
            "run_id": run_id,
            "owner_hotkey": owner,
            "device": DEVICE,
        }
        (m / "miner.toml").write_text("".join(f"{k} = {json.dumps(v)}\n" for k, v in conf.items()))
        (m / "origin.txt").write_text(origin_ids[i] + "\n")
        (m / "api.txt").write_text(api + "\n")
        (m / "run_id.txt").write_text(run_id + "\n")
    print(json.dumps({"miners": int(n), "dir": str(root)}))
    return 0


def status(root: Path, i: int) -> dict:
    for attempt in range(40):
        rc, out, err = cli(
            root, i, "status-v2", ["status-v2", "--config", "miner.toml"], attempt=attempt
        )
        if rc == 0:
            return json.loads(out)
        if not transient(out, err):
            raise SystemExit(f"status-v2 miner {i} rc={rc}: {err[-300:]}")
        time.sleep(3)
    raise SystemExit(f"status-v2 miner {i}: retries exhausted")


def join(root: Path, i: int) -> None:
    """join-v2 once, then escrow-lock once; both resume from their marker files."""
    m = root / f"m{i}"
    api, run_id = (m / "api.txt").read_text().strip(), (m / "run_id.txt").read_text().strip()
    if not (m / "join.json").exists():
        # One request id and expiry per identity, kept before the first call: a join whose
        # answer was lost replays the same signed request instead of a new one.
        request = m / "join-request.json"
        if not request.exists():
            request.write_text(
                json.dumps({"request_id": secrets.token_hex(32), "expiry": now_round(api) + 1000})
            )
        req = json.loads(request.read_text())
        for attempt in range(40):
            rc, out, err = cli(
                root,
                i,
                "join-v2",
                [
                    "join-v2",
                    "--config",
                    "miner.toml",
                    "--coldkey",
                    "cold.seed",
                    "--request-id",
                    req["request_id"],
                    "--expiry",
                    str(req["expiry"]),
                ],
                attempt=attempt,
            )
            if rc == 0:
                break
            if not transient(out, err):
                raise SystemExit(f"join-v2 miner {i}: {err[-500:]}")
            time.sleep(3)
        else:
            raise SystemExit(f"join-v2 miner {i}: retries exhausted")
        (m / "join.json").write_text(out)
    if (m / "lock.json").exists():
        return
    from hypertrain.miner.core import load_keyfile

    hot = load_keyfile(m / "hot.seed").ss58
    for attempt in range(40):
        rc, out, err = cli(
            root,
            i,
            "escrow-lock",
            [
                "escrow-lock",
                "--api",
                api,
                "--run-id",
                run_id,
                "--coldkey",
                "cold.seed",
                "--hotkey-ss58",
                hot,
                "--units",
                "1000",
                "--origin",
                (m / "origin.txt").read_text().strip(),
            ],
            attempt=attempt,
        )
        if rc == 0:
            (m / "lock.json").write_text(out)
            return
        if not transient(out, err):
            raise SystemExit(f"escrow-lock miner {i}: {out[-300:]} {err[-500:]}")
        time.sleep(3)
    raise SystemExit(f"escrow-lock miner {i}: retries exhausted")


def trial(root: Path, i: int, k: int) -> None:
    m = root / f"m{i}"
    proof = m / f"proof-{k}.json"
    for attempt in range(40):
        rc, out, err = cli(
            root, i, "probe-v2", ["probe-v2", "--config", "miner.toml"], trial=k, attempt=attempt
        )
        if rc == 0:
            proof.write_text(out)
            break
        if not transient(out, err):
            raise SystemExit(f"probe-v2 miner {i} trial {k}: {err[-800:]}")
        time.sleep(3)
    else:
        raise SystemExit(f"probe-v2 miner {i} trial {k}: retries exhausted")
    for attempt in range(40):
        rc, out, err = cli(
            root,
            i,
            "proof-v2",
            ["proof-v2", "--config", "miner.toml", "--proof", proof.name],
            trial=k,
            attempt=attempt,
        )
        if rc == 0:
            return
        if not transient(out, err):
            raise SystemExit(f"proof-v2 miner {i} trial {k}: {err[-800:]}")
        time.sleep(3)
    raise SystemExit(f"proof-v2 miner {i} trial {k}: retries exhausted")


def require_capability() -> None:
    if DEVICE == "cuda":
        # island_worker.py refuses a worker whose HT_IMAGE_DIGEST differs from the manifest's.
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", os.environ.get("HT_IMAGE_DIGEST", "")):
            raise SystemExit("set HT_IMAGE_DIGEST to the manifest image_digest (docker run -e)")
        return
    if not os.environ.get("ATEN_CPU_CAPABILITY"):
        raise SystemExit(
            "set ATEN_CPU_CAPABILITY to the challenge host's capability (avx2 on s100)"
        )


def admit(argv: list[str]) -> int:
    require_capability()
    root, plan = Path(argv[0]), [tuple(map(int, x.split(":"))) for x in argv[1].split(",")]
    for i, target in plan:
        if DEVICE == "cuda":
            wait_gpu_idle()  # the join starts the 100-round window of the first challenge
        join(root, i)
        while True:
            record = status(root, i)["record"]
            if record["clean_count"] >= target:
                log(
                    root,
                    {
                        "miner": i,
                        "stage": "target",
                        "state": record["state"],
                        "clean": record["clean_count"],
                    },
                )
                break
            before = record["clean_count"]
            trial(root, i, before + 1)
            after = status(root, i)["record"]
            if after["clean_count"] != before + 1:
                raise SystemExit(
                    f"miner {i}: clean {after['clean_count']} != {before + 1} "
                    f"({after['state']} {after.get('reason')})"
                )
    return 0


def round_(argv: list[str]) -> int:
    require_capability()
    root, w, miners = Path(argv[0]), int(argv[1]), [int(x) for x in argv[2:]]
    results: dict[int, str] = {}

    def one(i: int) -> None:
        # run-v2 is idempotent (accept, commit, grant and upload replay), so a transient
        # refusal is retried with the same command.
        for attempt in range(40):
            _, out, err = cli(
                root,
                i,
                "run-v2",
                ["run-v2", "--config", "miner.toml", "--round", str(w)],
                w=w,
                attempt=attempt,
            )
            results[i] = out.strip()[-200:]
            if '"UPLOADED"' in out or not transient(out, err):
                return
            time.sleep(3)

    # One miner at a time: on s100 the TEST relay BLS-verifies drand in now() on every
    # capability and chunk call, and four concurrent uploads (2026-10-26 22:29Z) pushed its
    # handlers past its own 2-round staleness bound, so every first chunk got 400. One by one,
    # each upload took about 12 s.
    for i in miners:
        one(i)
    ok = all('"UPLOADED"' in results.get(i, "") for i in miners)
    print(json.dumps({"w": w, "uploaded": ok, "results": results}))
    return 0 if ok else 1


if __name__ == "__main__":
    command, rest = sys.argv[1], sys.argv[2:]
    raise SystemExit({"setup": setup, "admit": admit, "round": round_}[command](rest))
