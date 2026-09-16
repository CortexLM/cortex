# Owner LIVE — the RLM-authorship install

**What this is.** The operator ceremony that moves a Proof topic onto the RLM-authored
behavior path: the topic's own RLM authors the whole set (rules, migrations, APIs, submission
format, pin policy) inside its topic VM, the install applies **that** instead of the
operator's bundle section, and the journal records `rlm` as the author of every part.

**Why it is the operator's, not an agent's.** It provisions a Firecracker VM and runs a paid
baseline against a live judge offer. An agent can prepare and verify everything up to the
first command; the spend and the Owner assertion are the Owner's.

**What it is not.** It is **not** a re-run of the B1 FIXED YAML. A topic's behavior comes from
`authoring.json` written inside the VM. An install driven from a bundle — however good the
bundle — records `topic_document` provenance, and the publish gate refuses to open a topic on
it. So this ceremony cannot be satisfied by a human-authored document, and the journal will
say so if one is tried.

---

## 0. Preconditions

| Need | Where it comes from | Refusal if missing |
|---|---|---|
| Topic-VM orchestrator | `PROOF_VM_ORCHESTRATOR_URL` (https, KVM host agent) | `--drive-rlm` refuses, naming the env var |
| Orchestrator bearer **file** | `PROOF_VM_ORCHESTRATOR_TOKEN_FILE` | same |
| RLM VM image digest | `PROOF_RLM_VM_IMAGE_DIGEST` (`sha256:…`) | same; a digest is never invented |
| Owner inference key file | `PROOF_RLM_OWNER_INFERENCE_KEY_FILE` (or `--owner-key-file`) | the lifecycle stops at `awaiting_owner_keys` |
| Live judge offer | `PROOF_INFERENCE_OFFER_FILE` (`--skip-baseline` avoids the need, and then no baseline is measured) | the driver refuses rather than binding a run to a placeholder |
| Master database | `BASE_DATABASE_URL` (or `_FILE`) | the install refuses |
| Operator bearer file | `--admin-token-file` (for the publish) | resolved before anything is written |
| A registered custom id | `PROOF_VM_RUNNER_CUSTOM_IDS` on the host | the install refuses an open topic whose id is not registered |
| **An adaptor whose `propose_rules` writes `authoring.json`** | the guest image, baked by the operator (`deploy/guest/bake-rootfs.sh`) | the run fails closed: a runner with no `propose_rules` at all is `NO_RLM_RULES`, and a rules-only adaptor answers a **fragment**, which the driver refuses (`IncompleteAuthoring`) |

**The guest image must be re-baked for the entrypoint to exist.** `propose_rules`
is an operator artefact: `bake-rootfs.sh --runner <id>=<dir>` copies the runner
tree into `/opt/proof/runners/<id>/` and chmods `run`, `inspect`, and
`propose_rules`. Tipping `proof-challenge` (or the gateway) does **not** update
`/opt/proof/runners`, so `--drive-rlm` keeps failing closed on the old pin.
Rebake, then set `PROOF_RLM_VM_IMAGE_DIGEST` to the new image's `sha256sum` —
never invent one. The reference adaptor ships the entrypoint at
[`deploy/guest/runners/rlm_fc_in_guest_harbor/propose_rules`](../../deploy/guest/runners/rlm_fc_in_guest_harbor/propose_rules)
(`harness/authoring_set.py` is the author). Verify the baked image carries it
before running the ceremony:

```bash
# The plan names the runner the bake will copy (and its tree must carry the entrypoint).
deploy/guest/bake-rootfs.sh --guest-agent <proof-vm-guest-agent> \
  --runner rlm_fc_in_guest_harbor="$(pwd)/deploy/guest/runners/rlm_fc_in_guest_harbor" \
  --overlay <harbor-venv> --chroot-hook <install-harbor.sh> \
  --resolver <allowlisted resolver> --out-dir ./out --dry-run
test -x deploy/guest/runners/rlm_fc_in_guest_harbor/propose_rules \
  || echo "the adaptor tree ships no propose_rules: --drive-rlm will fail closed"
```

**Re-authoring reads the previous set.** On a second authoring run the guest
writes the set the RLM authored last time to `$PROOF_WORK_DIR/current-authoring.json`
and exports its path as **`PROOF_CURRENT_AUTHORING_FILE`** (empty when there is
no previous set). An adaptor reads it to **retain** the parts it is not
changing — without it, a re-authoring run is a rewrite from nothing and a
migration the topic still needs would silently vanish. The set is stored per
topic (`proof_topic_authoring`, migration `0027`), so it survives a restart and
a different operator process is handed the same one.

**The last row is the one that changed.** An adaptor baked before this change writes
`rules.json` only. Its topic can still be installed, but it **cannot open**: the rules land
with honest `rlm` provenance and the driver stops, naming the four parts that have no author.

---

## 1. What the RLM must author

The adaptor's `propose_rules` entrypoint writes `$PROOF_OUTPUT_DIR/authoring.json`:

```json
{
  "schema_version": 1,
  "topic_id": "<the topic id this VM is bound to>",
  "rules": [
    {"id": "no_short_circuit", "text": "the harness must run the task"}
  ],
  "migrations": [
    {"name": "0001_scratch", "sql": "CREATE TABLE <topic>_scratch (id TEXT, note TEXT)"}
  ],
  "apis": [
    {"path": "status", "method": "GET", "summary": "topic status"}
  ],
  "submission_format": {"kind": "tar", "max_bytes": 5242880},
  "pin_policy": {}
}
```

**Every key is required.** `deny_unknown_fields`: a key this build does not read is refused
by name rather than ignored.

| Part | Rules it must satisfy | Refusal reads |
|---|---|---|
| `topic_id` | equals the VM's bound topic | `authoring.json is for topic "x", this VM is bound to "y"` |
| `rules` | the same shape the scoring path checks (id slug, non-empty text, no duplicates, ≤64) | `authoring.json: rules: checklist[id]: …` |
| `migrations` | names are ids; SQL is non-empty, ≤256 KiB, ≤64 migrations, and **inside this topic's namespace** under the same deny-list an operator's bundle faces | `authoring.json: migrations[0] ("0001_x"): …` — the statement and the object are named |
| `apis` | relative path of plain segments (no leading `/`, no `..`), **not** inside `v1/admin`, method ∈ {GET,POST,PUT,PATCH,DELETE,*} | `authoring.json: apis[0]: …` |
| `submission_format` | a non-empty object | `authored no submission_format` |
| `pin_policy` | may be `{}` (restates nothing). A knob it **does** set must match the signed document's own value: scoring reads the document, so a policy that named a different number would be a threshold no challenger is judged by. `eval_image_digest`, `gpu_class`, and `holdout_size` are equalities against the **pin** as well | `pin_policy.epsilon_nll_min = X diverges from the signed document's Y: scoring reads the document…` |

**Two processes check it, and they cannot disagree** (both link `proof-topic-authoring`):

- the **guest**, before the answer becomes a job output: shape, the deny-list, and the policy
  against the **document's own knobs** (the VM has no pin);
- the **control plane**, before anything is applied: the same shape checks plus the policy
  against the **global pin** (`set.validate_against_pin`).

---

## 2. The ceremony

```bash
# ── 1. The RLM authors, the install applies ITS set, then publishes ──────────
#    Staging first. `--drive-rlm` needs `--owner-approved`: it provisions a VM
#    and spends on a baseline. The install publishes LAST, so a failure here
#    leaves the topic unreachable rather than half-live.
PROOF_VM_ORCHESTRATOR_URL=https://<kvm-host>:8200 \
PROOF_VM_ORCHESTRATOR_TOKEN_FILE=/run/base/proof/vm_orchestrator_token \
PROOF_RLM_VM_IMAGE_DIGEST=sha256:<pinned> \
PROOF_RLM_OWNER_INFERENCE_KEY_FILE=/run/base/proof/owner_inference_key \
PROOF_INFERENCE_OFFER_FILE=/run/base/proof/inference_offer.json \
BASE_DATABASE_URL=postgres://<master> \
  proof-admin topic install \
    --bundle <bundle>.json \
    --env staging \
    --drive-rlm --owner-approved \
    --admin-url https://<master> \
    --admin-token-file /run/base/proof/admin_tokens

# ── 2. Read the measurement and the commitment the open document must seal ───
proof-admin topic baseline <topic-id>

# ── 3. Put that commitment into the document, set `status: open`, sign it ────
#    The `proof` key stays with the Owner: `xtask proof-topic` signs a draft.
#    Then seal and publish.
proof-admin topic seal <topic-id> \
  --document <open>.json --publish \
  --admin-url https://<master> \
  --admin-token-file /run/base/proof/admin_tokens

# ── 4. Prove it ─────────────────────────────────────────────────────────────
proof-admin topic install-log --topic <topic-id> --json | jq '.binding.authorship'
```

**Metal** is the same with `--env metal --owner-metal-ack` (which asserts an Owner authorized
it *and* that staging passed for this bundle). Nothing else changes.

---

## 2b. On `cortex-staging`, exactly

The ceremony above with the staging host's own paths. **The RLM-emitted set is the source of
truth** — a human-authored YAML (the B1 `tb4-b1-first5-FIXED.yaml`) is not the path here: it
produces `topic_document` provenance and the publish gate refuses to open the topic on it.

```bash
# ── 0. The guest image must carry propose_rules ─────────────────────────────
#    Rebake the runner tree into the image, then re-pin. Tipping the challenge
#    alone leaves /opt/proof/runners on the old pin and --drive-rlm fails closed.
ssh cortex-staging 'test -x /opt/proof/runners/rlm_fc_in_guest_harbor/propose_rules \
  && echo "propose_rules present" || echo "REBAKE REQUIRED"'

# ── 1. Migrations 0027 / 0028 on the staging database ───────────────────────
#    proof_topic_authoring (the stored sets) + the route-revision column and
#    the DELETE grant register_apis reconciles with.
sqlx migrate run --source crates/db/migrations      # 26 → 28

# ── 2. The RLM authors; the install applies ITS set ────────────────────────
#    --drive-rlm provisions the topic VM and spends on a baseline: staging first.
PROOF_VM_ORCHESTRATOR_URL=https://<kvm-host>:8200 \
PROOF_VM_ORCHESTRATOR_TOKEN_FILE=/run/base/proof/vm_orchestrator_token \
PROOF_RLM_VM_IMAGE_DIGEST=sha256:<rebaked> \
PROOF_RLM_OWNER_INFERENCE_KEY_FILE=/run/base/proof/owner_inference_key \
PROOF_INFERENCE_OFFER_FILE=/run/base/proof/inference_offer.json \
BASE_DATABASE_URL=postgres://<staging-master> \
  proof-admin topic install \
    --bundle <topic>.json \
    --env staging \
    --drive-rlm --owner-approved \
    --admin-url https://gateway.cortex.foundation/challenge/proof \
    --admin-token-file /run/base/proof/admin_tokens

# ── 3. Read the measurement, seal the open document, publish ───────────────
proof-admin topic baseline <topic-id>
proof-admin topic seal <topic-id> --document <open>.json --publish \
  --admin-url https://gateway.cortex.foundation/challenge/proof \
  --admin-token-file /run/base/proof/admin_tokens

# ── 4. The journal is the proof, per part ──────────────────────────────────
proof-admin topic install-log --topic <topic-id> --json \
  | jq '.binding.authorship.parts | to_entries[] | "\(.key): \(.value.source)"'
```

**What must read back** (the five parts, all `rlm`; see § 3 for the full shape):

```
rules: rlm
migrations: rlm
apis: rlm
submission_format: rlm
pin_policy: rlm
```

If `rules` alone is `rlm` and the rest are absent, the drive produced a **fragment**: the
baked adaptor wrote `rules.json`. Rebake with an adaptor whose `propose_rules` writes
`authoring.json` and re-run — the driver resumes rather than restarting.

**Clone-diff against the legacy `tbench` behavior.** The point of the ceremony is that the
topic's behavior is no longer the operator's YAML. Compare what landed against the B1 FIXED
run:

```sql
-- The rule vector in force, and who wrote it (v5–v7 were already rlm).
SELECT version, source, digest FROM proof_rule_version
 WHERE topic_id = '<topic-id>' ORDER BY version DESC LIMIT 5;

-- What the newest install applied, and the authorship it journalled.
SELECT id, state, rules_version, migrations, binding -> 'authorship' AS authorship
  FROM proof_topic_install WHERE topic_id = '<topic-id>' ORDER BY id DESC LIMIT 1;

-- The routes the topic exposes: the RLM's set, not the bundle's.
SELECT path, method FROM proof_topic_api WHERE topic_id = '<topic-id>' ORDER BY path;
```

The legacy `tbench` document carried a 15-task slice with 5 INFRA excludes and a compiled
rule list. The RLM's set instead carries the rules the **signed document declares** (framed
by the RLM, ticked by the signed `inspect_*` policy) and the migrations/routes/format/policy
the RLM authored — so the diff is expected to differ, and the journal is what says so.

---

## 3. What proves it worked

**Not the exit code — the journal.** The newest `proof_topic_install` row's
`binding.authorship` must read:

```json
{
  "source": "rlm",
  "digest": "<whole-set digest>",
  "parts": {
    "rules":             {"source": "rlm", "version": <v>, "digest": "sha256:…"},
    "migrations":        {"source": "rlm", "digest": "sha256:…", "names": ["0001_scratch"]},
    "apis":              {"source": "rlm", "digest": "sha256:…", "routes": ["GET /status"]},
    "submission_format": {"source": "rlm", "digest": "sha256:…"},
    "pin_policy":        {"source": "rlm", "digest": "sha256:…"}
  }
}
```

**Check, in order:**

1. `binding.authorship.source == "rlm"` — if it says `topic_document`, the RLM's set did not
   reach the install (the drive failed, or the adaptor wrote only `rules.json`).
2. Every `parts.*.source == "rlm"`, each with its own `digest`.
3. `parts.migrations.names` and `parts.apis.routes` name **what actually landed** — compare
   them to the tables in the database and the rows in `proof_topic_api`.
4. `binding.vms_per_submission == 1`.
5. `GET /v1/status` reports the topic in `scorable_topics` with `can_score: true`.

**The negative check, which is the point of the whole thing:** re-running the ceremony with a
human-authored bundle section and **no** `authoring.json` produces
`binding.authorship.source == "topic_document"`, and the publish step refuses to open the
topic. That is the property the B1 FIXED YAML did not have.

---

## 4. When it refuses

| Refusal | What it means | What to do |
|---|---|---|
| `IncompleteAuthoring { missing: [...] }` | the RLM authored rules and nothing else — an adaptor baked before the set existed | bake an adaptor whose `propose_rules` writes `authoring.json`, then re-run |
| `Authoring { why: "… loosens the floor/ceiling …" }` | the RLM's pin policy is looser than the global pin | the topic's policy is wrong; re-author (the refusal names the knob and both numbers) |
| `Section { part: "migrations[0]", why: "…proof_rule_version…" }` | the RLM's migration reached outside its namespace | the RLM's SQL is wrong; it is refused **before** anything runs |
| `CrossTopicClaim { … }` | a migration names an object another registered topic also claims | rename the object, or scope it with the schema-qualified spelling |
| `CustomIdNotRegistered { … }` | an open custom topic whose id this host does not score | register the id (`PROOF_VM_RUNNER_CUSTOM_IDS`) or fix the document |
| `VmError::NotWired` | no orchestrator configured | fix the env; nothing ran on the control-plane host |

**Everything above is pre-flight or pre-journal**: the refusals that could leave a partial
state write nothing at all, and the ones that happen after the journal opens append a
`failed` row naming the step. `proof-admin topic install-log --topic <id>` reads it back.

---

## 5. Rollback notes

- A failed install leaves the topic **unpublished**: publishing is the last step, so miners
  cannot reach it (no status, no route, no submission).
- Migrations already applied are recorded in `proof_topic_install`; a re-run skips them. They
  are **not** rolled back automatically — drop them by hand if the bundle is being replaced
  rather than fixed.
- Rules already installed stay installed; a re-run keeps the version it finds.
- A failed paid run retains its guest on the KVM host for root-cause analysis
  (`PROOF_VM_AGENT_RETAIN_DIR`), and the topic VM is kept.
- Re-running the same command **resumes** rather than restarts: the lifecycle and the journal
  are both persisted.
