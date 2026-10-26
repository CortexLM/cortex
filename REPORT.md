# S5 / D20: real GPU miner from B300 against the s100 TEST stack: REPORT

> **BLOCKER (16:00Z).** The chain join-v2 → probation → run-v2 with GPU training → audits →
> `get_weights` cannot run against the s100 TEST stack today. Following the brief, I stopped
> before writing anything to s100. Nothing on s100 or on the B300 hosts was changed.
>
> 1. **The TEST challenge image cannot ingest a B300 qualification.** `hypertrain-test-challenge`
>    runs `ghcr.io/cortexlm/hypertrain@sha256:a543239e…` (label revision `dc163190`, old
>    CortexLM/hypertrain repo, built 2026-10-20T22:06Z) [V, `docker image inspect` on s100]. Its
>    `gpu_ops/network_qualification.py:129` reads `sm_counts: list[Literal[170]]` [V, read-only
>    `docker exec … grep`]. Pydantic refuses a 148-SM B300 record before `store.py:2430`
>    (`any(s != ref.sm_count …)`) is reached. The fix is `8a90d9bc` (2026-10-26T13:46Z), and it
>    is not in `origin/v3` [V, `git merge-base`].
> 2. **Swapping the TEST challenge to the miner image does not fix it.** The miner image
>    `hypertrain-gpu@sha256:f4b2885f…` is built from `2f4406bd` (2026-10-26T12:09Z), which still
>    has `Literal[170]` at line 129 [V, label on host B, `git show 2f4406bd:…`]. A B300 CUDA
>    qualification record therefore needs an image rebuilt from a commit that contains
>    `8a90d9bc`. Neither `f09e6bd9` nor `f4b2885f` qualifies (see the workflow verdicts below).
>    The brief says: stop and report; do not swap.
> 3. **s100 has no GPU to compute CUDA references or audits.** Without role dispatch, the TEST
>    challenge computes trusted references and audit replays in-process with the run's backend.
>    The replay code says "CPU functional backend only: never a CUDA hardware claim"
>    (test-image `store.py` near 7240) [V]. Dispatch drives only Vast hosts (S4a,
>    `role_dispatch.py:102-107`) [A]. A CUDA run hosted on s100 has no reference compute, and
>    founder rule 15:4xZ forbids running the challenge on a rented host.
> 4. **No auditor audits the TEST stack.** `hypertrain-auditor-go-0/1` (`f09e6bd9`) poll
>    **production** `http://cortex-challenge-hypertrain:8000`, run `7753ff75…` [V, container
>    `Cmd`]. The old `hypertrain-auditor-0/1` (`58e2b427`) exited 5 days ago [V, `docker ps -a`].
>    The brief says to recreate nothing that reaches prod.
>
> **Decision needed (Forge / founder):** (a) build and pin a challenge image from a commit
> that contains `8a90d9bc` and swap the TEST challenge to it; (b) add a non-Vast GPU
> dispatch target, or accept a CPU-backend TEST run, in which case the miner does not train on
> the GPU; (c) start TEST-stack auditors. Steps (a) and (b) are code or ops work that cannot be
> done before 17:00Z.

Labels: [V] verified in this session, [A] reported by another session, [H] hypothesis, [?] unknown.

## Status

BLOCKED at step 3 of the method (touching the s100 TEST stack). The offline steps are in progress below.

## Result

Being filled in.

## Evidence

- s100 containers and images: read-only `docker ps -a`, `docker image inspect`, `docker inspect`
  (Env reduced to names), `docker exec hypertrain-test-challenge grep/sed` at 15:51–15:56Z.
- Host B (108.137.3.212) at 15:57Z: 0 compute apps, driver 595.91.07, only `executor-*`
  containers, image `f4b2885f` present with revision label `2f4406bd` [V].

## Risks

Being filled in.

## Not verified

Being filled in.

## Next

Being filled in.
