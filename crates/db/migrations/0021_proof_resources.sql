-- Provider responses are retained even if the dispatcher's lease expired.
-- Only the current controller can adopt resources or complete cleanup.
ALTER TABLE proof_experiment ADD CONSTRAINT proof_experiment_account_unique
    UNIQUE (id, account_id);
ALTER TABLE proof_service_intent ADD CONSTRAINT proof_intent_experiment_unique
    UNIQUE (id, experiment_id);

CREATE TABLE proof_resource (
    account_id      UUID NOT NULL,
    resource_id     TEXT NOT NULL CHECK (length(resource_id) BETWEEN 1 AND 256),
    experiment_id   UUID NOT NULL,
    intent_id       UUID NOT NULL,
    quote_id        UUID NOT NULL,
    status          TEXT NOT NULL DEFAULT 'quarantined'
                    CHECK (status IN ('quarantined', 'active', 'deleting', 'deleted')),
    authorized_until BIGINT NOT NULL CHECK (authorized_until > 0),
    deletion_id     UUID UNIQUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, resource_id),
    UNIQUE (intent_id, resource_id),
    FOREIGN KEY (experiment_id, account_id) REFERENCES proof_experiment (id, account_id),
    FOREIGN KEY (intent_id, experiment_id) REFERENCES proof_service_intent (id, experiment_id),
    FOREIGN KEY (experiment_id, quote_id) REFERENCES proof_machine_quote (experiment_id, id),
    CHECK (status NOT IN ('deleting', 'deleted') OR deletion_id IS NOT NULL)
);

CREATE TABLE proof_provider_observation (
    id              UUID PRIMARY KEY,
    intent_id       UUID NOT NULL REFERENCES proof_service_intent (id),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    result          JSONB NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE proof_deletion_observation (
    id              UUID PRIMARY KEY,
    account_id      UUID NOT NULL,
    resource_id     TEXT NOT NULL,
    deletion_id     UUID NOT NULL REFERENCES proof_resource (deletion_id),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    result          JSONB NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (account_id, resource_id) REFERENCES proof_resource (account_id, resource_id)
);

ALTER TABLE proof_service_intent ADD COLUMN dispatched_at TIMESTAMPTZ;
ALTER TABLE proof_experiment_event DROP CONSTRAINT proof_experiment_event_kind_check;
ALTER TABLE proof_experiment_event ADD CONSTRAINT proof_experiment_event_kind_check
    CHECK (kind IN ('created', 'quoted', 'approved', 'cancel_requested',
        'provision_dispatched', 'resource_adopted', 'cleanup_started', 'cleanup_verified'));

GRANT SELECT, INSERT ON proof_resource, proof_provider_observation, proof_deletion_observation
    TO base_app;
GRANT UPDATE (status, deletion_id) ON proof_resource TO base_app;
