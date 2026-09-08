-- Proof RLM: DB-backed topic versions, RLM-authored rule versions,
-- per-submission checklists, lifecycle transitions, artefact metadata, and
-- the promotion continuum (best pointer + history).
--
-- Proof is a dynamic agentic challenge system. Nothing about a challenge is
-- compiled into the binary: every row here is data a signed topic document
-- or that topic's RLM (running in its own VM) produced. Rules the RLM writes
-- land in `proof_rule_version`, versioned with the topic, so the gate a
-- submission was ticked against is replayable months later — not only in
-- logs. `proof_promotion_event` is the learning continuum: baseline → miner
-- runs → best → what the next run has to beat.
--
-- Append-only tables (`proof_rule_version`, `proof_checklist`,
-- `proof_lifecycle_event`, `proof_promotion_event`,
-- `proof_baseline_measurement`) get INSERT + SELECT only for `base_app`; the
-- current best pointer is the newest promotion row, never an UPDATE.
-- `topic_id` follows the topic slug CHECK; digests are lowercase 64 hex.

CREATE TABLE proof_topic_version (
    topic_id     TEXT NOT NULL,
    version      INTEGER NOT NULL,             -- 1, 2, … per re-sign
    status       TEXT NOT NULL,                -- draft | open | closed (document status)
    document     JSONB NOT NULL,               -- the signed topic document, verbatim
    signature    TEXT NOT NULL,                -- sr25519 hex over the canonical document
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, version),
    CONSTRAINT proof_topic_version_id_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_version_status_check CHECK (status IN ('draft', 'open', 'closed')),
    CONSTRAINT proof_topic_version_pos CHECK (version >= 1)
);

CREATE TABLE proof_rule_version (
    topic_id     TEXT NOT NULL,
    version      INTEGER NOT NULL,             -- 1 = the signed document's checklist vector
    source       TEXT NOT NULL,                -- topic_document | rlm | operator
    rules        JSONB NOT NULL,               -- [{"id","text"}], evaluation order
    digest       TEXT NOT NULL,                -- sha256 hex over the canonical rule set
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, version),
    CONSTRAINT proof_rule_version_source_check CHECK (source IN ('topic_document', 'rlm', 'operator')),
    CONSTRAINT proof_rule_version_digest_check CHECK (digest ~ '^[0-9a-f]{64}$'),
    CONSTRAINT proof_rule_version_pos CHECK (version >= 1)
);

CREATE TABLE proof_checklist (
    submission_digest TEXT PRIMARY KEY,        -- frozen submission digest
    topic_id          TEXT NOT NULL,
    rules_version     INTEGER NOT NULL,
    green             BOOLEAN NOT NULL,        -- complete and every rule passed
    failed_ids        JSONB NOT NULL,          -- ["rule_id", …], empty when green
    document          JSONB NOT NULL,          -- checklist.json verbatim
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_checklist_digest_check CHECK (submission_digest ~ '^[0-9a-f]{64}$')
);
CREATE INDEX proof_checklist_topic ON proof_checklist (topic_id, created_at);

CREATE TABLE proof_lifecycle_event (
    id           BIGSERIAL PRIMARY KEY,
    topic_id     TEXT NOT NULL,
    from_state   TEXT NOT NULL,
    event        TEXT NOT NULL,
    to_state     TEXT NOT NULL,
    note         TEXT NOT NULL DEFAULT '',     -- operator-readable, never a secret
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_lifecycle_event_states_check CHECK (
        from_state IN ('draft','owner_presend','awaiting_owner_keys','provisioning','baselining','open','evaluating','promoting','closed')
        AND to_state IN ('draft','owner_presend','awaiting_owner_keys','provisioning','baselining','open','evaluating','promoting','closed')
    )
);
CREATE INDEX proof_lifecycle_event_topic ON proof_lifecycle_event (topic_id, id);

CREATE TABLE proof_baseline_measurement (
    topic_id      TEXT NOT NULL,
    rules_version INTEGER NOT NULL,
    primary_value DOUBLE PRECISION NOT NULL,   -- what the RLM measured before any submission
    report        JSONB NOT NULL,              -- run report verbatim
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, rules_version)
);

CREATE TABLE proof_artefact (
    topic_id          TEXT NOT NULL,
    submission_id     TEXT NOT NULL,           -- store row id (pf_ + 16 hex)
    submission_digest TEXT NOT NULL,
    path              TEXT NOT NULL,           -- {root}/{topic_id}/{submission_id}.zip
    sha256            TEXT NOT NULL,           -- of the zip bytes
    bytes             BIGINT NOT NULL,
    primary_value     DOUBLE PRECISION,        -- NULL on a pre-spend reject
    checklist_green   BOOLEAN NOT NULL,
    promoted          BOOLEAN NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, submission_id),
    CONSTRAINT proof_artefact_row_id_check CHECK (submission_id ~ '^pf_[0-9a-f]{16}$'),
    CONSTRAINT proof_artefact_sha_check CHECK (sha256 ~ '^[0-9a-f]{64}$')
);

CREATE TABLE proof_promotion_event (
    id                BIGSERIAL PRIMARY KEY,
    topic_id          TEXT NOT NULL,
    submission_id     TEXT NOT NULL,
    submission_digest TEXT NOT NULL,
    primary_value     DOUBLE PRECISION NOT NULL,
    bar               DOUBLE PRECISION,        -- what it had to beat (sealed or previous best)
    previous_best     TEXT,                    -- displaced submission_id, NULL for the first crown
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_promotion_event_row_id_check CHECK (submission_id ~ '^pf_[0-9a-f]{16}$')
);
CREATE INDEX proof_promotion_event_topic ON proof_promotion_event (topic_id, id);

GRANT SELECT, INSERT ON TABLE proof_topic_version TO base_app;
GRANT SELECT, INSERT ON TABLE proof_rule_version TO base_app;
GRANT SELECT, INSERT ON TABLE proof_checklist TO base_app;
GRANT SELECT, INSERT ON TABLE proof_lifecycle_event TO base_app;
GRANT USAGE, SELECT ON SEQUENCE proof_lifecycle_event_id_seq TO base_app;
GRANT SELECT, INSERT ON TABLE proof_baseline_measurement TO base_app;
GRANT SELECT, INSERT ON TABLE proof_artefact TO base_app;
GRANT SELECT, INSERT ON TABLE proof_promotion_event TO base_app;
GRANT USAGE, SELECT ON SEQUENCE proof_promotion_event_id_seq TO base_app;
