# `proof-admin` dry-run fixture — Owner A→Z

Operator dry-run artifact for the dynamic-topics install path (P0 + P1a).
**Nothing here is a production topic.**

| File | What it is |
|------|------------|
| `tb4.install-bundle.json` | A **Topic Install Bundle**: slug `tb4`, alias `tbench`, install target `staging`, carrying a signed `TopicDocument`, an `rlm` install section (`rules`, `migrations`, `apis`, `submission_format`, `scoring`), and the Owner-default alias. |
| `tb4.pin.toml` | The `ProofPin` that document is checked against. |

## Exact commands

Run from the repository root:

```bash
cargo run -p proof-admin-bin -- topic validate \
  --bundle bins/proof-admin/tests/fixtures/tb4.install-bundle.json \
  --pin bins/proof-admin/tests/fixtures/tb4.pin.toml

cargo run -p proof-admin-bin -- topic install \
  --bundle bins/proof-admin/tests/fixtures/tb4.install-bundle.json \
  --env staging --dry-run \
  --pin bins/proof-admin/tests/fixtures/tb4.pin.toml
```

`--bin proof-admin` works too and is package-name-agnostic:

```bash
cargo run --bin proof-admin -- topic validate \
  --bundle bins/proof-admin/tests/fixtures/tb4.install-bundle.json \
  --pin bins/proof-admin/tests/fixtures/tb4.pin.toml
```

Both write nothing and need no database.

## Running the install for real

Drop `--dry-run` and supply the master and the operator bearer:

```bash
cargo run -p proof-admin-bin -- topic install \
  --bundle bins/proof-admin/tests/fixtures/tb4.install-bundle.json \
  --env staging \
  --pin bins/proof-admin/tests/fixtures/tb4.pin.toml \
  --admin-url http://127.0.0.1:8100 \
  --admin-token-file /run/proof/admin_token
```

This fixture's document is signed by the **test** key, so a real install
against a live master would be refused at the publish step. Use it to exercise
the gates and the dry run; the real `tb4` document is signed by the `proof`
row key and is a follow-up (see below).

Add `--drive-rlm --owner-approved` to provision the topic VM and run the paid
baseline; that step needs a wired topic-VM orchestrator and spends, so it is
the Owner's call, not a walkthrough step. `--skip-baseline` stops before the
baseline job.

Read the journal back with:

```bash
BASE_DATABASE_URL=… proof-admin topic install-log --topic tb4
```

## The rest of the path to a scorable topic

An install never makes a topic scorable on its own, whichever way it runs: the
topic needs an **`open`** document whose baseline is **sealed**, and the seal
is the operator's. `topic install` prints this, and `--json` reports it as
`"scorable": false` with the `remaining` steps. The three commands are:

```bash
# 1. Measure the baseline (paid: a VM + a judge call). Requires the
#    topic-VM orchestrator and the owner's assertions.
proof-admin topic install --bundle <bundle> --env staging --drive-rlm --owner-approved \
  --admin-url <master-or-gateway> --admin-token-file <file>

# 2. Read what was measured and the commitment the open document must seal.
proof-admin topic baseline tb4

# 3. Put that `metrics_commitment` into the document, set `status: open`, sign
#    it with the `proof` key (`xtask proof-topic`), then seal and publish.
proof-admin topic seal tb4 --document <open.json> --publish \
  --admin-url <master-or-gateway> --admin-token-file <file>
```

`topic seal` runs the same `mark_sealed` the runtime uses, so a document it
accepts is one the scoring path accepts; `--publish` then posts it through the
admin route (which still refuses an `open` document whose install is not
`applied`). Confirm with `GET /v1/status`: `can_score` is true and the topic is
in `scorable_topics`.

`--skip-baseline` is a **pause**, not a path: it installs the rules and leaves
the topic unscorable until a run without the flag measures a baseline.

### Two things the command needs

**`-p proof-admin-bin`, not `-p proof-admin`.** The repo names binary packages
with a `-bin` suffix (`trustroot-bin` → `trustroot`, `validator-bin` →
`validator`), so the package is `proof-admin-bin` and the *binary* is
`proof-admin`. `cargo run --bin proof-admin -- …` sidesteps the distinction.

### `--pin` is required, and here is why

The fixture is signed with the **test mini-secret** the CLI tests use, so it
must be checked against the fixture pin. Omitting `--pin` falls back to
`config/proof-pin.toml`, which carries the **real** proof trust root:

```
$ cargo run -p proof-admin -- topic validate \
    --bundle bins/proof-admin/tests/fixtures/tb4.install-bundle.json
proof-admin: topic signature: topic signature does not verify under the proof trust-root key
```

That refusal is the signature check working correctly — a test-signed document
is not this subnet's topic. The real `tb4` document is signed by the `proof`
row key and is a follow-up (see below).

## What this fixture is not

- **Not a production topic.** The document is signed with a test mini-secret;
  `tb4.pin.toml` carries the matching `topic_pubkey`.
- **Not a real RLM install.** The `rlm` section is a small illustrative sample
  (`rules`, `migrations`, `apis`, `submission_format`, `scoring`). A real
  bundle carries the topic's own — which the RLM consumes and this repository
  never interprets. The sample's migration (`CREATE TABLE tb4_scratch`) is
  legal under the deny-list precisely because it stays inside the topic's own
  namespace.
- **Not the metal artifact.** The metal signed Operator `tb4.json` is a
  follow-up; this fixture exists so the staging A→Z walkthrough can exercise
  `validate`, `--dry-run`, and the install gates today.

## Staging migrate

`crates/db/migrations/0024_proof_topic_alias.sql`,
`crates/db/migrations/0025_proof_topic_install.sql`, and
`crates/db/migrations/0026_proof_topic_gate.sql` are the schema changes in
this stack.

**There is no manual migration command to run.** Migrations are embedded in
the `db` crate (`sqlx::migrate!("./migrations")`) and applied automatically on
boot wherever `BASE_DATABASE_URL` is set — the gateway does this. So the
staging path is the **service restart**:

```bash
# Restart the master services with BASE_DATABASE_URL set (compose / remote-deploy).
# Migrations apply on boot; no separate step.
```

`cargo sqlx migrate run` is **not** an option in this repo: `sqlx-cli` is not
a workspace dependency and is not installed in a clean checkout, so that
command fails with `error: no such command: sqlx`.

What they do, exactly:

- `0024` **adds** `proof_topic_alias` (`alias → topic_id`, plus a `topic_id`
  index). It is a mapping and nothing else — no display name, no pins, no
  status, no document; those stay in `proof_topic_version` (migration `0020`).
  It also adds `BEFORE INSERT` (and `UPDATE` on the alias table) triggers,
  `proof_topic_alias_no_shadow` and `proof_topic_version_no_shadow`, which
  make an alias collision with a published slug fail closed in **both**
  directions. That is a publish-path integrity guard, not scoring math.
- `0025` **adds** `proof_topic_install` (the install journal: bundle digest,
  environment, state, rules version, rule ids, migrations applied, executor
  binding, detail) and `proof_topic_api` (the routes a topic registers, with
  paths stored **relative** so a row cannot escape the topic's prefix). Both
  are append-only for `base_app`: a journal that could be edited in place
  would not be a journal, so a re-install appends.
- `0026` **adds** `proof_topic_gate` (the operator switch `topic disable` /
  `topic enable` appends to: state, reason, actor). Append-only too — the
  newest row per topic is the state, the rows before it are the history — and
  the challenge reads it on the submit path, so a disable takes effect on the
  next request with no re-sign, restart, or redeploy.
- None of them **does** `ALTER` or `DROP` anything: the `0020` tables keep
  their columns, keys, and grants.

## Regenerating

The bundle's document must verify under its pin, so both files come from the
same test fixture the CLI tests use. Do not hand-edit them:

```bash
PROOF_ADMIN_FIXTURE_DIR=bins/proof-admin/tests/fixtures \
  cargo test -p proof-admin-bin --test cli regenerate_dry_run_fixture
```

`the_committed_dry_run_fixture_still_validates_and_plans` runs both documented
commands on every test run, so a schema change that breaks this fixture fails
CI rather than reaching the Owner.
