-- Proof topic authoring: what a topic's RLM authored, so a **re-authoring**
-- run can be given the set it wrote last time.
--
-- A topic's behavior is five parts (rules, migrations, APIs, submission
-- format, pin policy) and its own RLM authors all of them in one job
-- (`authoring.json`). The rules have always been versioned in
-- `proof_rule_version`; the other four had nowhere to live, so a second
-- authoring run had no way to see what the first one produced. That is a
-- correctness problem, not a convenience: an adaptor that cannot read its
-- previous set cannot *retain* the parts it is not changing, so a re-authoring
-- run silently drops migrations a topic still needs — and the install would
-- apply that lossy set.
--
-- This table is the missing half: one row per authored set, newest last, so
-- the driver can hand the RLM the set it wrote before
-- (`VmJob::ProposeRules.current`) and the adaptor can keep what it means to
-- keep.
--
-- The rows are a **journal**, like every other `proof_*` table: an
-- authoring run appends, nothing is rewritten, and "the set in force" is the
-- newest row for a topic. `digest` is the canonical digest of the document
-- stored beside it, so an audit can prove the row is the set it claims to be
-- rather than a re-serialisation of it.
--
-- `topic_id` is the shared challenge DB's discriminant, exactly like every
-- other `proof_*` table; there is no per-topic schema. The id shape is the
-- same CHECK `proof_topic_version` enforces.

CREATE TABLE proof_topic_authoring (
    id         BIGSERIAL PRIMARY KEY,
    topic_id   TEXT NOT NULL,
    -- Monotonic per topic, starting at 1: which authoring run this is.
    version    INTEGER NOT NULL,
    -- The set verbatim, as the RLM wrote it.
    document   JSONB NOT NULL,
    -- `sha256:<64 lowercase hex>` of the document's canonical JSON. Never
    -- invented: the driver computes it from the set it read back.
    digest     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_authoring_topic_check
        CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_authoring_version_pos CHECK (version >= 1),
    CONSTRAINT proof_topic_authoring_digest_check
        CHECK (digest ~ '^sha256:[0-9a-f]{64}$'),
    -- One row per (topic, version): a re-run of the same authoring is the
    -- same set, so a duplicate is a mistake rather than history.
    CONSTRAINT proof_topic_authoring_unique UNIQUE (topic_id, version)
);

-- The read is "the newest set for this topic" — what the next authoring run
-- is handed.
CREATE INDEX ix_proof_topic_authoring_topic
    ON proof_topic_authoring (topic_id, version DESC);

-- Append-only for the application role: a journal that could be edited in
-- place would not be a journal. A re-authoring appends.
GRANT SELECT, INSERT ON TABLE proof_topic_authoring TO base_app;
GRANT USAGE, SELECT ON SEQUENCE proof_topic_authoring_id_seq TO base_app;
