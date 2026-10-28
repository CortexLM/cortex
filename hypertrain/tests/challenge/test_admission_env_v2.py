"""ADR 0011 part 3: the server compares the environment a miner declares with the manifest's pin
and refuses a difference with a typed error before it spends anything, at join and at the trial.

The declaration is a body the miner signed (the dual-signed JoinRequest, the WorkScreenV2 of the
trial). A refusal is raised before the first write, so no admission, quota, strike, suspension or
nonce is consumed and the same party can fix its host and ask again. The trial test runs a real
reference and a real miner round (two torchrun launches), then edits the declaration the way a
divergent host would report it.
"""

from __future__ import annotations

import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).parent))
from test_admission_v2 import (  # noqa: E402
    COLD,
    HOT,
    apply,
    current_proof,
    request,
    service,
    setup,
    submit,
)

from env_pin_fixtures import CPU_AVX2, honest_observation
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EnvObservation, HardwareHint, WorkProof

Edit = Callable[[dict[str, Any]], None]


def env(name: str, value: str | None) -> Edit:
    return lambda seen: seen["env"].__setitem__(name, value)


# One divergence each: what the Principal named (ISA, MKL/ONEDNN value, sm, attention backend, FP8
# flag) plus the neighbours that share a code path. The expected detail names the differing names.
DIVERGENCES: dict[str, tuple[Edit, str]] = {
    "isa-resolved": (lambda seen: seen.__setitem__("cpu_capability", "AVX512"), "cpu_capability"),
    "isa-name": (env("ATEN_CPU_CAPABILITY", "avx512"), "ATEN_CPU_CAPABILITY"),
    "mkl": (env("MKL_CBWR", "COMPATIBLE"), "MKL_CBWR"),
    "mkl-unset": (env("MKL_CBWR", None), "MKL_CBWR"),
    "onednn": (env("ONEDNN_MAX_CPU_ISA", "ALL"), "ONEDNN_MAX_CPU_ISA"),
    "sm": (lambda seen: seen.__setitem__("gpu", "sm_103"), "gpu"),
    "attention": (env("HT_ATTN_BACKEND", "sdpa-flash"), "HT_ATTN_BACKEND"),
    "fp8": (env("HT_FP8", "1"), "HT_FP8"),
    "thread-limit": (env("OMP_THREAD_LIMIT", "2"), "OMP_THREAD_LIMIT"),
    "name-outside-the-pin": (env("LD_PRELOAD", "/x"), "LD_PRELOAD"),
}


def declared(edit: Edit | None = None) -> EnvObservation:
    seen = honest_observation(CPU_AVX2)
    if edit is not None:
        edit(seen)
    return EnvObservation.model_validate(seen)


def join_with(a, s, env: EnvObservation):
    req = sign_join(
        HOT,
        COLD,
        run_id=s.manifest.run_id(),
        request_id=request(s).request_id,
        expires_beacon=10000,
        policy_hash=request(s).policy_hash,
        hardware_hint=HardwareHint(device_name="advisory", device_count=999, driver="cpu-test"),
        env=env,
    )
    return a.join(canonicalize(req.body()), now=1, ip_prefix="loopback")


def rows(a) -> tuple[int, int]:
    db = a.store.db
    return (
        db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0],
        db.execute("SELECT COUNT(*) FROM admission_quota").fetchone()[0],
    )


@pytest.mark.parametrize("fault", sorted(DIVERGENCES))
def test_a_join_declaring_a_diverging_environment_is_refused_and_consumes_nothing(
    tmp_path: Path, fault: str
) -> None:
    s = setup()
    a = service(s, tmp_path)
    edit, names = DIVERGENCES[fault]
    before = rows(a)
    with pytest.raises(AdmissionError) as refused:
        join_with(a, s, declared(edit))
    assert str(refused.value) == f"ENV_PIN_DIVERGES: {names}"
    assert rows(a) == before == (0, 0)
    # The same identity, request and quota are intact: the honest declaration is accepted.
    assert join_with(a, s, declared()).state == "APPLIED"


def test_a_join_declaring_every_difference_names_them_all(tmp_path: Path) -> None:
    s = setup()
    a = service(s, tmp_path)
    seen = honest_observation(CPU_AVX2)
    seen["cpu_capability"], seen["gpu"] = "AVX512", "sm_120"
    seen["env"]["HT_FP8"], seen["env"]["MKL_CBWR"] = "1", None
    with pytest.raises(
        AdmissionError, match="ENV_PIN_DIVERGES: HT_FP8, MKL_CBWR, cpu_capability, gpu"
    ):
        join_with(a, s, EnvObservation.model_validate(seen))


@pytest.fixture(scope="module")
def trial(tmp_path_factory: pytest.TempPathFactory):
    path = tmp_path_factory.mktemp("env-admission")
    s = setup()
    a = service(s, path)
    admission = apply(a, s)
    a.challenge(admission, now=2)
    a.prepare_reference(admission, now=2)
    proof, screen = current_proof(a, admission, 2)
    return a, admission, proof, screen


def state(a, admission: str) -> tuple:
    row = a.store.db.execute(
        "SELECT state,strikes,pending_dispute,proof_digest FROM admissions_v2 WHERE admission_id=?",
        (admission,),
    ).fetchone()
    outcomes = a.store.db.execute("SELECT outcome FROM admission_trial_results").fetchall()
    return (*tuple(row), tuple(o[0] for o in outcomes))


# "conforming" convicts the module's trial: it must stay the last case.
@pytest.mark.parametrize("fault", [*sorted(DIVERGENCES), "conforming"])
def test_a_trial_declaring_a_diverging_environment_is_refused_and_convicts_nothing(
    trial, fault: str
) -> None:
    a, admission, proof, screen = trial
    before = state(a, admission)
    assert before[:3] == ("APPLIED", 0, 0)
    # A divergent host also mints other bytes: the proof below would be a MISMATCH if it were
    # compared. Refusing the declaration comes first, so nothing is convicted.
    wrong = WorkProof.model_validate({**proof.body(), "leaves_root": "ab" * 32})
    if fault == "conforming":
        # Control: the same wrong bytes under a conforming declaration are still the verdict.
        convicted = submit(a, wrong, screen)
        assert (convicted.state, convicted.pending_dispute) == ("SUSPENDED", True)
        assert state(a, admission)[1] == 1
        return
    edit, names = DIVERGENCES[fault]
    seen = screen.env.model_dump()
    edit(seen)
    diverged = screen.model_copy(update={"env": EnvObservation.model_validate(seen)})
    with pytest.raises(AdmissionError) as refused:
        submit(a, wrong, diverged)
    assert str(refused.value) == f"ENV_PIN_DIVERGES: {names}"
    assert state(a, admission) == before


def test_the_genuine_proof_is_accepted_after_the_refusals(tmp_path: Path) -> None:
    # Own fixture: the module trial above ends convicted by its control case.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.challenge(admission, now=2)
    a.prepare_reference(admission, now=2)
    proof, screen = current_proof(a, admission, 2)
    for edit, names in DIVERGENCES.values():
        seen = screen.env.model_dump()
        edit(seen)
        diverged = screen.model_copy(update={"env": EnvObservation.model_validate(seen)})
        with pytest.raises(AdmissionError, match=f"ENV_PIN_DIVERGES: {names}"):
            submit(a, proof, diverged)
    accepted = submit(a, proof, screen)
    assert (accepted.state, accepted.strikes, accepted.pending_dispute) == ("PROBATION", 0, False)
