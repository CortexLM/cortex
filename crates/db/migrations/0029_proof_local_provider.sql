-- Durable local CPU capacity slots for the opt-in proof-experiment service.
-- A slot is a controller-owned unit of one local Docker daemon; claiming one
-- is the only "rental" the local provider performs. Zero-cost quotes stay an
-- accounting placeholder; nothing is ever charged.
CREATE TABLE proof_local_slot (
    engine_id TEXT NOT NULL CHECK (engine_id <> ''),
    slot INTEGER NOT NULL CHECK (slot >= 0),
    intent_id UUID NULL UNIQUE,
    resource_id TEXT NULL UNIQUE CHECK (resource_id ~ '^[A-Za-z0-9_-]+$' AND length(resource_id) <= 256),
    PRIMARY KEY (engine_id, slot),
    CHECK ((intent_id IS NULL) = (resource_id IS NULL))
);
CREATE TABLE proof_local_event (
    id UUID PRIMARY KEY,
    engine_id TEXT NOT NULL CHECK (engine_id <> ''),
    image_id TEXT NOT NULL CHECK (image_id ~ '^sha256:[0-9a-f]{64}$'),
    intent_id UUID NOT NULL,
    account_id UUID NOT NULL,
    slot INTEGER NULL CHECK (slot >= 0),
    resource_id TEXT NULL CHECK (resource_id ~ '^[A-Za-z0-9_-]+$' AND length(resource_id) <= 256),
    kind TEXT NOT NULL CHECK (kind IN ('claimed', 'refused', 'released')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (intent_id, kind),
    CHECK ((kind = 'refused') = (slot IS NULL AND resource_id IS NULL))
);
-- One provider request id resolves to exactly one creation outcome forever.
CREATE UNIQUE INDEX proof_local_event_outcome
    ON proof_local_event (intent_id) WHERE kind IN ('claimed', 'refused');
GRANT SELECT, INSERT ON proof_local_slot, proof_local_event TO base_app;
GRANT UPDATE (intent_id, resource_id) ON proof_local_slot TO base_app;
