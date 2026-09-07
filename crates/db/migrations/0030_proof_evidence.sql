-- Append-only, exact-bytes public evidence store. The signature stays inside
-- `wire`; `signature` is duplicated for auditing and never re-derived.
CREATE TABLE gateway_proof_evidence (
    evidence_digest TEXT PRIMARY KEY CHECK (evidence_digest ~ '^[0-9a-f]{64}$'),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    signature TEXT NOT NULL CHECK (signature ~ '^[0-9a-f]{128}$'),
    wire BYTEA NOT NULL CHECK (octet_length(wire) BETWEEN 1 AND 4096),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
GRANT SELECT, INSERT ON gateway_proof_evidence TO base_app;
