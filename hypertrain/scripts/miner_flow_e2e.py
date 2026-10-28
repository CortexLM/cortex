"""CPU end-to-end proof of the seedless miner flow + synchronized update. No GPU, no Vast.

Operator HOME creates a coldkey-signed server key; only the bundle reaches the "server" HOME;
`hypertrain-miner install` runs there; the key is listed, then revoked (403). Two miners train
tiny real rounds, a signed update lands with changeover ahead, both swap at the same round
boundary after checkpointing, resume, and every round replays bitwise (MATCH).
Writes evidence JSON to $MINER_FLOW_EVIDENCE (default ./miner-flow-evidence.json); exit 0 = PASS.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))

from hypertrain.protocol.env_pins import EnvPin, class_pin, pinned_environ  # noqa: E402

# ATen, MKL and oneDNN read their variables at first use, and every manifest pins them: a plain
# container exports none, so this process takes the pin of the config it trains before torch loads.
PIN = EnvPin.from_body(class_pin("avx2", "none"))
_pinned = pinned_environ(os.environ, PIN)
os.environ.clear()
os.environ.update(_pinned)

import hypertrain.trainer  # noqa: E402, F401, I001  # isort: skip  (determinism must precede torch)
import numpy as np  # noqa: E402
import torch  # noqa: E402

from challenge.conftest import (  # noqa: E402
    ADMIN,
    COORD_SEED,
    INTERNAL,
    WORKER,
    Master,
    make_client,
)
from hypertrain.miner import update  # noqa: E402
from hypertrain.miner.keys import REGISTRY_ENV, create_key, list_keys, revoke_key  # noqa: E402
from hypertrain.protocol.keys import Keypair  # noqa: E402
from hypertrain.trainer.config import (  # noqa: E402
    CompressConfig,
    InnerConfig,
    ModelConfig,
    TrainConfig,
)
from hypertrain.trainer.loop import Assignment, train_round  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402

COLD_SEED = b"\x42" * 32
RELEASE = Keypair(bytes(range(32)))
WHEEL = b"hypertrain-0.2.0 test wheel"
T0 = 1_800_000_000
ROUND_S = 5
WHEEL_URL = "https://example.test/hypertrain-0.2.0-py3-none-any.whl"
ROUNDS = 6


def check(ok: bool, what: str, ev: dict[str, Any]) -> None:
    ev["checks"][what] = bool(ok)
    if not ok:
        raise AssertionError(what)


def keys_flow(tmp: Path, ev: dict[str, Any]) -> None:
    os.environ[REGISTRY_ENV] = str(tmp / "operator" / ".hypertrain" / "keys.json")
    secrets = tmp / "secrets"
    secrets.mkdir()
    for name, value in (
        ("internal.token", INTERNAL),
        ("admin.token", ADMIN),
        ("worker.token", WORKER),
        ("coord.key", COORD_SEED.hex()),
    ):
        (secrets / name).write_text(value)
    cold_path = tmp / "operator" / "cold.key"
    cold_path.parent.mkdir(parents=True)
    cold_path.write_text(COLD_SEED.hex())
    cold_path.chmod(0o600)
    cold = Keypair(COLD_SEED)
    master = Master()
    with make_client(tmp / "state", secrets, master, [float(T0)]) as client:
        bundle = create_key(cold_path, "vast-1", client, out_dir=tmp / "operator" / "bundle")
        server_home = tmp / "server"
        shutil.copytree(bundle, server_home / "bundle")
        blobs = b"".join(p.read_bytes() for p in server_home.rglob("*") if p.is_file())
        check(
            COLD_SEED.hex().encode() not in blobs and COLD_SEED not in blobs,
            "server_home_has_no_coldkey",
            ev,
        )
        server = Keypair(bytes.fromhex((server_home / "bundle" / "server.key").read_text()))
        cfg = server_home / "miner.toml"
        r = subprocess.run(  # noqa: S603
            [
                sys.executable,
                "-m",
                "hypertrain.miner.cli",
                "install",
                "--bundle",
                str(server_home / "bundle"),
                "--config",
                str(cfg),
                "--api",
                "http://challenge.test",
                "--workdir",
                str(server_home / "work"),
            ],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "HOME": str(server_home)},
        )
        check(r.returncode == 0 and cfg.exists(), "cli_install_on_server", ev)
        listed = list_keys(cold.ss58, client)
        check(any(k.get("server_pub") == server.ss58 for k in listed), "key_listed", ev)
        master.registered.add(server.ss58)
        body = {"w": 0, "hotkey": server.ss58, "sha256": "0" * 64}
        before = client.post("/v1/runs/unknown/uploads", json=body).status_code
        key_id = next(k["key_id"] for k in listed if k.get("server_pub") == server.ss58)
        revoke_key(cold_path, key_id, client)
        after = client.post("/v1/runs/unknown/uploads", json=body).status_code
        check(before != 403 and after == 403, "revoked_key_gets_403", ev)
        ev["keys"] = {"server": server.ss58, "status_before": before, "status_after": after}


def tiny_cfg() -> TrainConfig:
    return TrainConfig(
        model=ModelConfig(
            n_layers=1,
            d_model=16,
            n_heads=2,
            n_kv_heads=2,
            d_ff=24,
            vocab=32,
            seq_len=4,
            n_experts=0,
        ),
        inner=InnerConfig(lr=0.003, H=2, J=1, micro_batch=1, state_policy="reset"),
        compress=CompressConfig("dense-int8", ef_beta=0),
        env_pin=PIN,
        n_stages=1,
    )


def sample(i: int) -> np.ndarray:
    return (np.arange(5, dtype=np.uint32) + i) % 32


def miner(
    idx: int, root: Path, signed: bytes, cfg: TrainConfig, ev: dict[str, Any]
) -> list[dict[str, Any]]:
    files = {"m": signed, WHEEL_URL: WHEEL}

    def install(wheel: Path, dest: Path) -> None:
        (dest / "venv").mkdir()

    up = update.Updater(root, "0.1.0", fetch=files.__getitem__, install=install)
    up.poll("m")
    up.stage()
    theta = init_params(cfg.model)
    version, log = "0.1.0", []
    for w in range(ROUNDS):
        now = T0 + w * ROUND_S
        if up.due(now):
            ckpt = root / "ckpt.pt"
            torch.save(theta, ckpt)
            up.swap()
            version = "0.2.0"
            up = update.Updater(root, version, fetch=files.__getitem__, install=install)
            theta = torch.load(ckpt)
        check(not up.must_halt(now) or version == "0.2.0", f"miner{idx}_not_halted_w{w}", ev)
        res = train_round(
            cfg, theta, Assignment("11" * 32, w, (w * 2 + idx, w * 2 + idx + 1)), sample
        )
        theta = {k: v.clone() for k, v in theta.items()}
        log.append(
            {
                "w": w,
                "version": version,
                "leaves_root": res.leaves_root,
                "final_theta_hash": res.final_theta_hash,
            }
        )
    return log


def update_flow(tmp: Path, ev: dict[str, Any]) -> None:
    os.environ[update.TEST_PUBKEY_ENV] = RELEASE.public.hex()
    changeover = T0 + 2 * ROUND_S + 1
    manifest = update.UpdateManifest(
        version="0.2.0",
        tag="v0.2.0",
        commit="a" * 40,
        wheel_url=WHEEL_URL,
        wheel_sha256=hashlib.sha256(WHEEL).hexdigest(),
        announce_at=changeover - 600,
        changeover_at=changeover,
    )
    signed = update.sign_manifest(manifest, RELEASE)
    cfg = tiny_cfg()
    logs = [miner(i, tmp / f"miner{i}", signed, cfg, ev) for i in (0, 1)]
    swaps = [next(r["w"] for r in log if r["version"] == "0.2.0") for log in logs]
    check(swaps[0] == swaps[1] and T0 + swaps[0] * ROUND_S >= changeover, "synchronized_swap", ev)
    check(
        all((tmp / f"miner{i}" / "current").is_symlink() for i in (0, 1)), "current_symlinked", ev
    )
    theta0 = init_params(cfg.model)
    match = True
    for i, log in enumerate(logs):
        for r in log:
            res = train_round(
                cfg,
                theta0,
                Assignment("11" * 32, r["w"], (r["w"] * 2 + i, r["w"] * 2 + i + 1)),
                sample,
            )
            match &= (res.leaves_root, res.final_theta_hash) == (
                r["leaves_root"],
                r["final_theta_hash"],
            )
    check(match, "replay_MATCH_across_swap", ev)
    ev["update"] = {
        "changeover_at": changeover,
        "swap_round": swaps,
        "rounds": logs,
        "replay": "MATCH" if match else "MISMATCH",
    }


def main() -> int:
    ev: dict[str, Any] = {"checks": {}}
    out = Path(os.environ.get("MINER_FLOW_EVIDENCE", "miner-flow-evidence.json"))
    code = 0
    with tempfile.TemporaryDirectory() as d:
        try:
            keys_flow(Path(d), ev)
            update_flow(Path(d), ev)
        except AssertionError as e:
            ev["failed"] = str(e)
            code = 1
    ev["result"] = "PASS" if code == 0 else "FAIL"
    out.write_text(json.dumps(ev, indent=1, sort_keys=True))
    print(json.dumps({"result": ev["result"], "checks": ev["checks"]}, sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
