-- Durable orchestration for Atlas / Proof v2. Historical challenge tables and
-- scoring are unchanged. Credential references are opaque keystore ids, not keys.

CREATE TABLE proof_miner_account (
    id              UUID PRIMARY KEY CHECK (id <> '00000000-0000-0000-0000-000000000000'),
    miner_hotkey    TEXT NOT NULL CHECK (miner_hotkey ~ '^[0-9a-f]{64}$'),
    credential_ref  UUID NOT NULL UNIQUE
                    CHECK (credential_ref <> '00000000-0000-0000-0000-000000000000'),
    revoked         BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, miner_hotkey)
);

CREATE TABLE proof_experiment (
    id              UUID PRIMARY KEY CHECK (id <> '00000000-0000-0000-0000-000000000000'),
    miner_hotkey    TEXT NOT NULL,
    account_id      UUID NOT NULL,
    recipe_digest   TEXT NOT NULL CHECK (recipe_digest ~ '^[0-9a-f]{64}$'),
    state           TEXT NOT NULL CHECK (state IN (
                        'discussion', 'awaiting_consent', 'approved', 'provisioning',
                        'running', 'collecting', 'cancelling', 'deleting',
                        'reconciling', 'completed', 'cancelled', 'rejected')),
    revision        BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    current_quote   UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (account_id, miner_hotkey) REFERENCES proof_miner_account (id, miner_hotkey)
);

CREATE TABLE proof_machine_quote (
    id                  UUID PRIMARY KEY CHECK (id <> '00000000-0000-0000-0000-000000000000'),
    experiment_id       UUID NOT NULL REFERENCES proof_experiment (id),
    experiment_revision BIGINT NOT NULL CHECK (experiment_revision > 0),
    digest              TEXT NOT NULL UNIQUE CHECK (digest ~ '^[0-9a-f]{64}$'),
    quote               JSONB NOT NULL CHECK (jsonb_typeof(quote) = 'object'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (experiment_id, id),
    UNIQUE (experiment_id, experiment_revision),
    UNIQUE (id, digest)
);

ALTER TABLE proof_experiment ADD CONSTRAINT proof_experiment_current_quote_fk
    FOREIGN KEY (id, current_quote) REFERENCES proof_machine_quote (experiment_id, id);

CREATE TABLE proof_quote_consent (
    quote_id        UUID PRIMARY KEY,
    quote_digest    TEXT NOT NULL,
    signature       TEXT NOT NULL CHECK (signature ~ '^[0-9a-f]{128}$'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (quote_id, quote_digest) REFERENCES proof_machine_quote (id, digest)
);

CREATE TABLE proof_action_nonce (
    miner_hotkey    TEXT NOT NULL CHECK (miner_hotkey ~ '^[0-9a-f]{64}$'),
    nonce           UUID NOT NULL CHECK (nonce <> '00000000-0000-0000-0000-000000000000'),
    body_digest     TEXT NOT NULL CHECK (body_digest ~ '^[0-9a-f]{64}$'),
    action          JSONB NOT NULL CHECK (jsonb_typeof(action) = 'object'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (miner_hotkey, nonce)
);

CREATE TABLE proof_experiment_event (
    experiment_id   UUID NOT NULL REFERENCES proof_experiment (id),
    revision        BIGINT NOT NULL CHECK (revision >= 0),
    kind            TEXT NOT NULL CHECK (kind IN (
                        'created', 'quoted', 'approved', 'cancel_requested',
                        'provision_dispatched')),
    state           TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (experiment_id, revision)
);

-- Never delete ownership rows: the fencing high-water mark survives release.
CREATE TABLE proof_controller_lease (
    experiment_id   UUID PRIMARY KEY REFERENCES proof_experiment (id),
    owner_id        UUID NOT NULL CHECK (owner_id <> '00000000-0000-0000-0000-000000000000'),
    fence           BIGINT NOT NULL CHECK (fence > 0),
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Persist intent before any external request. A dispatched rent becomes
-- reconcile on takeover; it must never automatically return to pending.
CREATE TABLE proof_service_intent (
    id              UUID PRIMARY KEY CHECK (id <> '00000000-0000-0000-0000-000000000000'),
    experiment_id   UUID NOT NULL REFERENCES proof_experiment (id),
    kind            TEXT NOT NULL CHECK (kind IN ('provision', 'cancel')),
    quote_id        UUID,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'dispatched', 'reconcile', 'completed', 'cancelled')),
    controller_fence BIGINT CHECK (controller_fence > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (experiment_id, quote_id) REFERENCES proof_machine_quote (experiment_id, id),
    CHECK (kind <> 'provision' OR quote_id IS NOT NULL),
    UNIQUE (quote_id, kind)
);

CREATE UNIQUE INDEX proof_service_single_cancel
    ON proof_service_intent (experiment_id) WHERE kind = 'cancel';
CREATE INDEX proof_service_pending ON proof_service_intent (experiment_id, status);

GRANT SELECT, INSERT, UPDATE ON TABLE
    proof_miner_account, proof_experiment, proof_controller_lease, proof_service_intent
TO base_app;
GRANT SELECT, INSERT ON TABLE
    proof_machine_quote, proof_quote_consent, proof_action_nonce, proof_experiment_event
TO base_app;
