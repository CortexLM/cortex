CREATE SEQUENCE proof_submission_id_seq;
GRANT USAGE ON SEQUENCE proof_submission_id_seq TO base_app;

-- Durable Proof submission and score journal.
--
-- `proof-challenge` served submissions and scores from memory, so a restart
-- silently discarded every scored run. These tables are the write-through
-- record the service reloads at startup.
CREATE TABLE proof_submission (
    id TEXT PRIMARY KEY CHECK (id ~ '^pf_[0-9a-f]{16}$'),
    topic_id TEXT NOT NULL CHECK (length(topic_id) BETWEEN 1 AND 128),
    miner_hotkey TEXT NOT NULL CHECK (length(miner_hotkey) BETWEEN 1 AND 128),
    artifact_digest TEXT NOT NULL CHECK (artifact_digest ~ '^[0-9a-f]{64}$'),
    -- Full row as served, so a reload reproduces exactly what was scored.
    document JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX proof_submission_topic ON proof_submission (topic_id);
CREATE INDEX proof_submission_miner ON proof_submission (miner_hotkey);

-- One best attempt per (miner, topic); payout reads the latest state.
CREATE TABLE proof_topic_run (
    miner_hotkey TEXT NOT NULL CHECK (length(miner_hotkey) BETWEEN 1 AND 128),
    topic_id TEXT NOT NULL CHECK (length(topic_id) BETWEEN 1 AND 128),
    pass BOOLEAN NOT NULL,
    primary_value DOUBLE PRECISION,
    artifact_digest TEXT NOT NULL DEFAULT '',
    near_duplicate BOOLEAN NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (miner_hotkey, topic_id),
    -- Reject NaN and infinities: they are never a real measurement.
    CHECK (
        primary_value IS NULL
        OR (primary_value <> 'NaN'::float8 AND primary_value <> 'Infinity'::float8
            AND primary_value <> '-Infinity'::float8)
    )
);

GRANT SELECT, INSERT, UPDATE ON proof_submission TO base_app;
GRANT SELECT, INSERT, UPDATE ON proof_topic_run TO base_app;
