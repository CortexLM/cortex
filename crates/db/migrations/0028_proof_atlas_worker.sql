-- Durable Atlas runtime identity and original budget deadline.
CREATE TABLE proof_atlas_runtime (
    round BIGINT PRIMARY KEY REFERENCES proof_atlas_round(round),
    id UUID NOT NULL UNIQUE,
    binding TEXT NOT NULL CHECK (binding ~ '^[0-9a-f]{64}$'),
    frozen_digest TEXT NOT NULL CHECK (frozen_digest ~ '^[0-9a-f]{64}$'),
    deadline_ms BIGINT NOT NULL CHECK (deadline_ms > 0),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    phase TEXT NOT NULL CHECK (phase IN ('started', 'finished', 'failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE proof_atlas_runtime_event (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES proof_atlas_runtime(id),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    kind TEXT NOT NULL CHECK (kind IN ('started', 'resumed', 'finished', 'failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (run_id, controller_fence, kind)
);
GRANT SELECT, INSERT ON proof_atlas_runtime, proof_atlas_runtime_event TO base_app;
GRANT UPDATE (controller_fence, phase) ON proof_atlas_runtime TO base_app;
