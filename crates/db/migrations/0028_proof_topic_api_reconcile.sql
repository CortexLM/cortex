-- Reconcile a topic's dynamic routes on re-install, and make the mux see it.
--
-- `proof_topic_api` was `SELECT, INSERT` for the application role, which made
-- it append-only — and a **row count** the mux's change signal. That was sound
-- while a topic's route set could only grow, but it is **wrong now that a set
-- can be replaced**: when an RLM-authored install supersedes a
-- bundle-authored one, the old set's routes are not in the new set, and an
-- append-only table leaves them resolving. A miner would still reach an
-- endpoint the topic's current install does not declare, while the journal
-- says the newer set is in force.
--
-- Two changes, and they belong together:
--
-- 1. The application role may `DELETE` from `proof_topic_api`. The install
--    reconciles the topic's rows against the set it is applying **inside one
--    transaction** with the insert, so a reader never sees a half-replaced
--    table. It stays without UPDATE: a route row is a claim, and a claim is
--    replaced rather than edited.
-- 2. The mux's change signal stops being a row count. A count cannot see a
--    replacement — delete three, insert three, and it is unchanged — so the
--    install bumps a **revision** instead: one monotonic number per topic,
--    incremented in the same transaction as the reconciliation. The mux sums
--    the revisions, which only ever grows, so a replacement moves the
--    generation exactly as an addition does.
--
-- The journal still keeps the history: `proof_topic_install` records every
-- install and `binding.authorship` says which set was in force. What this
-- migration stops is the *route table* accumulating claims from sets that are
-- no longer the topic's.

-- The install reconciles by deleting rows absent from the set it is applying.
GRANT DELETE ON TABLE proof_topic_api TO base_app;

-- One monotonic revision per topic, bumped by every install that writes routes
-- (and by a re-install that writes none, because the *reconciliation* is what
-- a reader has to see). A counter, not a journal: its history is the install
-- journal's business, and the only thing anyone reads here is the number.
CREATE TABLE proof_topic_route_revision (
    topic_id   TEXT PRIMARY KEY,
    revision   BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_route_revision_topic_check
        CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_route_revision_pos CHECK (revision >= 0)
);

-- The install's bump: `revision + 1`, and the row is created on first write.
-- This is the one place an UPDATE is granted, and it is a counter rather than
-- a record of what happened.
GRANT SELECT, INSERT, UPDATE ON TABLE proof_topic_route_revision TO base_app;
