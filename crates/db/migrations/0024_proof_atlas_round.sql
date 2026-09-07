CREATE TABLE proof_atlas_round (
    round BIGINT PRIMARY KEY CHECK (round >= 0),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    document JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_atlas_lease (
    round BIGINT PRIMARY KEY REFERENCES proof_atlas_round (round),
    owner UUID NOT NULL,
    fence BIGINT NOT NULL CHECK (fence > 0),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_atlas_decision (
    round BIGINT PRIMARY KEY REFERENCES proof_atlas_round (round),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    decision JSONB NOT NULL,
    history JSONB NOT NULL,
    chain_epoch BIGINT NOT NULL CHECK (chain_epoch > 0),
    leaves BYTEA NOT NULL,
    leaves_digest TEXT NOT NULL CHECK (leaves_digest ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_atlas_publication (
    round BIGINT PRIMARY KEY REFERENCES proof_atlas_decision (round),
    fence BIGINT NOT NULL DEFAULT 0 CHECK (fence >= 0),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    delivered BOOLEAN NOT NULL DEFAULT FALSE,
    confirmed_digest TEXT CHECK (confirmed_digest ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (NOT delivered OR confirmed_digest IS NOT NULL)
);
GRANT SELECT, INSERT ON proof_atlas_round, proof_atlas_decision, proof_atlas_lease, proof_atlas_publication TO base_app;
GRANT UPDATE (owner, fence, expires_at) ON proof_atlas_lease TO base_app;
GRANT UPDATE (fence, expires_at, delivered, confirmed_digest) ON proof_atlas_publication TO base_app;
