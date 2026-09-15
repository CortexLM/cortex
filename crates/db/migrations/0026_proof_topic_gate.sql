-- Proof topic gate: the operator switch that stops a topic taking submissions.
--
-- A topic's lifecycle lives in its **signed document** (`proof_topic_version`,
-- migration 0020): `draft` / `open` / `closed`. Moving that status is a signing
-- ceremony — the operator re-signs with the `proof` key — and an incident does
-- not wait for one. This table is the other switch: an operator at the CLI
-- records "this topic is disabled", and the challenge refuses every submission
-- to it on the next request, with no re-sign, no restart, and no redeploy.
--
-- It is a **journal**, not a state column, and for the same reason
-- `proof_topic_install` is one: the newest row for a topic is its current
-- gate state, and the rows before it are the history of who turned it off,
-- when, and why. A `disable` after an `enable` is a new row; nothing is
-- rewritten. An `enabled` row is what clears a disable — there is no DELETE,
-- because "who turned it back on" is exactly what an incident review asks.
--
-- What a disabled topic does *not* do: it does not un-sign, un-publish, or
-- re-score anything. The document keeps its status, in-flight evaluations
-- finish, and rows already scored keep their verdicts. The gate is a
-- **submission** gate: `POST /v1/submissions` answers 403 with the reason
-- before the nonce is spent, and the topic is not advertised as open for work.
-- Emission is untouched, so a topic disabled mid-epoch cannot silently break
-- the leaf a seal depends on.
--
-- Fail-closed at the reader: the challenge reads this table on the submit
-- path, and a read that fails is a **503**, never an admission. A host with no
-- database has no gate (and no published topics either).
--
-- `topic_id` is the shared challenge DB's discriminant, exactly like every
-- other `proof_*` table; there is no per-topic schema. The id shape is the
-- same CHECK `proof_topic_version` enforces.

CREATE TABLE proof_topic_gate (
    id         BIGSERIAL PRIMARY KEY,
    topic_id   TEXT NOT NULL,
    -- `disabled` stops submissions; `enabled` clears a disable and is the
    -- only way back, so the history is readable in one direction.
    state      TEXT NOT NULL,
    -- Why, in the operator's words. Shown to a miner in the 403, so it must
    -- never carry a secret; it is operator-authored text, not a key.
    reason     TEXT NOT NULL DEFAULT '',
    -- Who, for the incident review: an operator label, never a token.
    actor      TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_gate_topic_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_gate_state_check CHECK (state IN ('disabled', 'enabled')),
    CONSTRAINT proof_topic_gate_reason_check CHECK (length(reason) <= 512),
    CONSTRAINT proof_topic_gate_actor_check CHECK (length(actor) <= 128)
);

-- The read on the submit path is "the newest gate row for this topic"; the
-- operator listing is "newest first per topic".
CREATE INDEX ix_proof_topic_gate_topic ON proof_topic_gate (topic_id, id DESC);

-- Append-only for the application role: a gate that could be edited in place
-- would not be a history, and the runtime read is the newest row.
GRANT SELECT, INSERT ON TABLE proof_topic_gate TO base_app;
GRANT USAGE, SELECT ON SEQUENCE proof_topic_gate_id_seq TO base_app;
