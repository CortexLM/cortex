-- Untrusted narrative is deliberately separate from scientific observations.
CREATE TABLE proof_agent_report (
    id UUID PRIMARY KEY,
    experiment_id UUID NOT NULL REFERENCES proof_experiment (id),
    controller_fence BIGINT NOT NULL CHECK (controller_fence > 0),
    body TEXT NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
GRANT SELECT, INSERT ON proof_agent_report TO base_app;
