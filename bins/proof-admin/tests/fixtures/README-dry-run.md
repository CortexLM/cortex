# `proof-admin` dry-run fixture — Owner A→Z

Operator dry-run artifact for the dynamic-topics P0 skeleton (PR #297).
**Nothing here is a production topic.**

| File | What it is |
|------|------------|
| `tb4.install-bundle.json` | A **Topic Install Bundle**: slug `tb4`, alias `tbench`, install target `staging`, carrying a signed `TopicDocument` and an RLM install section. |
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
  (`rules`, `submission_format`). A real bundle carries the topic's own rules,
  migrations, APIs, submission format, and scoring — which the RLM consumes
  and this repository never interprets.
- **Not the metal artifact.** The metal signed Operator `tb4.json` is a
  follow-up; this fixture exists so the staging A→Z walkthrough can exercise
  `validate` and `--dry-run` today.

## Staging migrate

`crates/db/migrations/0024_proof_topic_alias.sql` is the **only** schema
change in this PR. Apply it through the usual sqlx path — migrations are
embedded and run automatically wherever `BASE_DATABASE_URL` is set (the
gateway does this on boot), so a compose / `remote-deploy` restart is the
path. There is no separate manual `sqlx migrate` step in this repo's deploy
flow:

```bash
cargo sqlx migrate run   # or: restart the service with BASE_DATABASE_URL set
```

What it does, exactly:

- **Adds** `proof_topic_alias` (`alias → topic_id`, plus a `topic_id` index).
  It is a mapping and nothing else — no display name, no pins, no status, no
  document; those stay in `proof_topic_version` (migration `0020`).
- **Adds** `BEFORE INSERT` (and `UPDATE` on the alias table) triggers,
  `proof_topic_alias_no_shadow` and `proof_topic_version_no_shadow`, which
  make an alias collision with a published slug fail closed in **both**
  directions. This is a **publish-path integrity guard, not scoring math** —
  it cannot change a score, a payout, or a sealed vector.
- **Does not** `ALTER` or `DROP` anything: the `0020` tables keep their
  columns, keys, and grants.

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
