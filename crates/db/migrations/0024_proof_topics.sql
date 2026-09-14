-- Proof topics: the installed-topic registry (dynamic-topics P0 skeleton).
--
-- Until now a Proof topic existed only as a signed document (`proof_topic_version`,
-- migration 0020) plus a set of host env vars. The dynamic-topics work moves the
-- per-topic bindings — which runner, which RLM image, which experiment pack, how
-- much concurrency, whether the topic is live — into the shared challenge DB,
-- keyed by `topic_id`. This table is that home. P0 lands the table and the admin
-- CLI skeleton; nothing reads it on a scoring path yet (no route change, no
-- allocator change), so adding it cannot move a score.
--
-- Relationship to `proof_topic_version`: that table is the append-only journal of
-- *signed documents* (a re-sign is a new version). This table is the single
-- current *install* row per topic — what the operator installed, from which
-- bundle, and whether it is enabled. `topic_id` is the discriminant and the
-- primary key: one row per topic, replaced in place on re-install.
--
-- The pin/binding columns mirror bindings that today travel in the signed
-- topic's `constraints.params` or in operator env (`in_guest_benchmark_runner`,
-- `experiment_pack_digest`, `PROOF_RLM_VM_IMAGE_DIGEST`, the experiment guest
-- image). They are install state here, not a second scoring contract: P0 writes
-- nothing and no scoring path reads them. Empty string means "not pinned", which
-- every later slice must read as fail-closed (an unpinned topic never boots),
-- never as "use a default".
--
-- `sealed_custom_value` stays NULL until the seal path measures the baseline. A
-- topic with no sealed value cannot be enabled, because nobody is paid for
-- beating a number nobody measured.
--
-- `aliases` is the Arch default for the first topic: the slug is `tb4` and
-- `tbench` is an alias, so old miner links keep resolving to one row rather
-- than two topics that could drift apart. Nothing resolves an alias yet (P0
-- has no route change); the column exists so the later slice does not need a
-- second migration.
--
-- No secrets: no key, token, or credential column. The CHECKs are shape guards
-- (slug, `sha256:<64 hex>`, finite baseline), never authentication.
--
-- Mutable table (enable/disable, re-install, seal), so `base_app` gets UPDATE —
-- but not DELETE: a topic is disabled, never dropped, so the install history a
-- bundle digest pins stays readable.

CREATE TABLE proof_topic (
    topic_id            TEXT PRIMARY KEY,
    display_name        TEXT NOT NULL,
    version             INTEGER NOT NULL,
    environment         TEXT NOT NULL,             -- staging | metal (install target)
    runner_id           TEXT NOT NULL DEFAULT '',  -- in-guest runner id, '' = none
    aliases             TEXT[] NOT NULL DEFAULT '{}', -- extra slugs the topic answers to
    enabled             BOOLEAN NOT NULL DEFAULT FALSE,
    config              JSONB NOT NULL DEFAULT '{}'::jsonb,
    pin_rlm             TEXT NOT NULL DEFAULT '',  -- sha256:<hex> RLM VM image
    pin_experiment      TEXT NOT NULL DEFAULT '',  -- sha256:<hex> experiment guest image
    pack_digest         TEXT NOT NULL DEFAULT '',  -- sha256:<hex> experiment pack tar
    n_concurrent        INTEGER NOT NULL DEFAULT 1,
    sealed_custom_value DOUBLE PRECISION,          -- NULL until the baseline is sealed
    schema_version      INTEGER NOT NULL,          -- install bundle schema version
    bundle              JSONB NOT NULL,            -- the validated bundle, verbatim
    bundle_digest       TEXT NOT NULL,             -- sha256:<hex> over canonical bundle
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_id_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_display_name_check CHECK (char_length(display_name) BETWEEN 1 AND 128),
    CONSTRAINT proof_topic_version_pos CHECK (version >= 1),
    CONSTRAINT proof_topic_environment_check CHECK (environment IN ('staging', 'metal')),
    CONSTRAINT proof_topic_runner_id_check
        CHECK (runner_id = '' OR runner_id ~ '^[a-z0-9][a-z0-9_-]{1,63}$'),
    -- Aliases are topic slugs too, and a topic is never its own alias.
    --
    -- The shape check is element-wise on purpose. `array_to_string` **drops
    -- NULL elements**, so a joined-string regex would happily accept
    -- `{tbench,NULL}` — and the typed reader decodes every element as a
    -- `String`, so that one accepted row would make `topic list` and
    -- `topic show` fail for the whole table. `array_position(..., NULL)` is
    -- the NULL probe that actually holds; it is separate from the regex so
    -- each constraint fails for one reason.
    CONSTRAINT proof_topic_aliases_bound CHECK (cardinality(aliases) <= 8),
    CONSTRAINT proof_topic_aliases_no_null CHECK (array_position(aliases, NULL) IS NULL),
    CONSTRAINT proof_topic_aliases_shape CHECK (
        cardinality(aliases) = 0
        OR array_to_string(aliases, ',') ~ '^[a-z0-9][a-z0-9-]{1,62}(,[a-z0-9][a-z0-9-]{1,62})*$'
    ),
    CONSTRAINT proof_topic_aliases_not_self CHECK (NOT (topic_id = ANY (aliases))),
    CONSTRAINT proof_topic_pin_rlm_check
        CHECK (pin_rlm = '' OR pin_rlm ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT proof_topic_pin_experiment_check
        CHECK (pin_experiment = '' OR pin_experiment ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT proof_topic_pack_digest_check
        CHECK (pack_digest = '' OR pack_digest ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT proof_topic_n_concurrent_pos CHECK (n_concurrent >= 1),
    CONSTRAINT proof_topic_config_object CHECK (jsonb_typeof(config) = 'object'),
    -- A baseline nobody measured is not a baseline: NaN / ±Infinity are refused
    -- here as well as in the bundle schema, because a non-finite value would
    -- silently lose every comparison the payout rule makes.
    CONSTRAINT proof_topic_sealed_value_finite CHECK (
        sealed_custom_value IS NULL
        OR (
            sealed_custom_value <> 'NaN'::float8
            AND sealed_custom_value <> 'Infinity'::float8
            AND sealed_custom_value <> '-Infinity'::float8
        )
    ),
    CONSTRAINT proof_topic_schema_version_pos CHECK (schema_version >= 1),
    CONSTRAINT proof_topic_bundle_digest_check CHECK (bundle_digest ~ '^sha256:[0-9a-f]{64}$')
);

-- The only read a later slice needs on the hot path: "the enabled topics".
CREATE INDEX ix_proof_topic_enabled ON proof_topic (enabled, topic_id);

-- Alias lookup ("is this slug a topic?") is a GIN scan, not a table walk.
CREATE INDEX ix_proof_topic_aliases ON proof_topic USING GIN (aliases);

GRANT SELECT, INSERT, UPDATE ON TABLE proof_topic TO base_app;
