-- Only trusted controller code has this pool. None of these tables is an
-- agent-facing execution or evidence-admission API.
CREATE TABLE proof_execution_target (
    experiment_id UUID PRIMARY KEY REFERENCES proof_experiment (id),
    account_id UUID NOT NULL,
    resource_id TEXT NOT NULL,
    engine_id TEXT NOT NULL,
    image_id TEXT NOT NULL CHECK (image_id ~ '^sha256:[0-9a-f]{64}$'),
    FOREIGN KEY (account_id, resource_id) REFERENCES proof_resource (account_id, resource_id)
);
CREATE TABLE proof_execution_script (
    digest TEXT PRIMARY KEY CHECK (digest ~ '^[0-9a-f]{64}$'),
    bytes BYTEA NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 32768)
);
CREATE TABLE proof_execution_intent (
    id UUID PRIMARY KEY,
    experiment_id UUID NOT NULL REFERENCES proof_experiment (id),
    operation_key TEXT NOT NULL CHECK (operation_key ~ '^[0-9a-f]{64}$'),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    owner_id UUID NOT NULL,
    resource_id TEXT NOT NULL,
    engine_id TEXT NOT NULL,
    image_id TEXT NOT NULL,
    plan JSONB NOT NULL,
    deadline_ms BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'dispatched'
        CHECK (state IN ('dispatched', 'reconcile', 'failed', 'completed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (experiment_id, operation_key)
);
CREATE UNIQUE INDEX proof_execution_one_active ON proof_execution_intent (experiment_id)
    WHERE state IN ('dispatched', 'reconcile');
CREATE TABLE proof_execution_observation (
    id UUID PRIMARY KEY,
    intent_id UUID NOT NULL REFERENCES proof_execution_intent (id),
    controller_fence BIGINT NOT NULL,
    observation JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE proof_execution_artifact (
    intent_id UUID NOT NULL REFERENCES proof_execution_intent (id),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    bytes BYTEA NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 1048576),
    PRIMARY KEY (intent_id, digest)
);
GRANT SELECT, INSERT ON proof_execution_target, proof_execution_script,
    proof_execution_intent, proof_execution_observation, proof_execution_artifact TO base_app;
GRANT UPDATE (state) ON proof_execution_intent TO base_app;
