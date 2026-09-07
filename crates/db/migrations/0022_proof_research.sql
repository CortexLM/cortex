CREATE TABLE proof_scientific_recipe (
    digest TEXT PRIMARY KEY CHECK (digest ~ '^[0-9a-f]{64}$'),
    recipe JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_scientific_evidence (
    digest TEXT PRIMARY KEY CHECK (digest ~ '^[0-9a-f]{64}$'),
    experiment_id UUID NOT NULL UNIQUE REFERENCES proof_experiment (id),
    recipe_digest TEXT NOT NULL REFERENCES proof_scientific_recipe (digest),
    evidence JSONB NOT NULL,
    public_summary JSONB NOT NULL,
    public_digest TEXT NOT NULL CHECK (public_digest ~ '^[0-9a-f]{64}$'),
    passed BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_evidence_artifact (
    evidence_digest TEXT NOT NULL REFERENCES proof_scientific_evidence (digest),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    bytes BYTEA NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 1048576),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (evidence_digest, digest)
);
CREATE TABLE proof_publication (
    evidence_digest TEXT PRIMARY KEY REFERENCES proof_scientific_evidence (digest),
    fence BIGINT NOT NULL DEFAULT 0 CHECK (fence >= 0),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    delivered BOOLEAN NOT NULL DEFAULT FALSE,
    confirmed_digest TEXT CHECK (confirmed_digest ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (NOT delivered OR confirmed_digest IS NOT NULL)
);
GRANT SELECT, INSERT ON proof_scientific_recipe, proof_scientific_evidence,
    proof_evidence_artifact TO base_app;
GRANT SELECT, INSERT ON proof_publication TO base_app;
GRANT UPDATE (fence, expires_at, delivered, confirmed_digest) ON proof_publication TO base_app;
