CREATE TABLE proof_work_schedule (
    experiment_id UUID PRIMARY KEY REFERENCES proof_experiment(id),
    next_wake TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    revision BIGINT NOT NULL CHECK (revision >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE proof_runtime_run (
    experiment_id UUID PRIMARY KEY REFERENCES proof_experiment(id),
    id UUID NOT NULL UNIQUE,
    binding TEXT NOT NULL CHECK (binding ~ '^[0-9a-f]{64}$'),
    deadline_ms BIGINT NOT NULL CHECK (deadline_ms > 0),
    phase TEXT NOT NULL CHECK (phase IN ('started', 'finished', 'failed')),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE proof_runtime_event (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES proof_runtime_run(id),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    kind TEXT NOT NULL CHECK (kind IN ('started', 'resumed', 'finished', 'failed', 'interrupted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
GRANT SELECT, INSERT ON proof_work_schedule, proof_runtime_run, proof_runtime_event TO base_app;
GRANT UPDATE (next_wake, revision) ON proof_work_schedule TO base_app;
GRANT UPDATE (phase, controller_fence) ON proof_runtime_run TO base_app;

ALTER TABLE proof_experiment_event DROP CONSTRAINT proof_experiment_event_kind_check;
ALTER TABLE proof_experiment_event ADD CONSTRAINT proof_experiment_event_kind_check
CHECK (kind IN ('created', 'quoted', 'approved', 'cancel_requested',
    'provision_dispatched', 'resource_adopted', 'cleanup_started', 'cleanup_verified',
    'quote_expired', 'quote_refresh'));
