-- Proof topic aliases: the temporary compatibility slug a topic answers to.
--
-- Owner default: the first topic's slug is `tb4`, with `tbench` as a
-- **temporary** alias, so existing miner links keep resolving while the
-- canonical slug moves. This table is that mapping and nothing else.
--
-- Why this is not a second topic table: a row here is `alias -> topic_id`.
-- No display name, no pins, no status, no document — every one of those lives
-- in `proof_topic_version` (migration 0020), which stays the single source of
-- truth for what a topic *is*. An alias cannot drift from the topic it names
-- because it carries no topic data to drift. Deleting the row retires the
-- alias; nothing else changes.
--
-- Shared challenge DB, `topic_id` discriminant: the same table serves every
-- topic in the one database, exactly like `proof_topic_version`. There is no
-- per-topic schema anywhere in this design.
--
-- No foreign key: `proof_topic_version` is keyed `(topic_id, version)`, so
-- `topic_id` alone is not unique and cannot be an FK target. Resolution is
-- therefore fail-closed in the store instead — an alias whose topic has no
-- published version resolves to *nothing*, never to an empty document. The
-- alias CHECKs are shape guards only.
--
-- `alias` is a topic slug (`[a-z0-9][a-z0-9-]{1,62}`), matching the id shape
-- `proof_topic_version` enforces, and an alias may never be its own topic's
-- id: that would be a second spelling of the same key in one lookup.
--
-- Mutable and **deletable**, unlike the journal tables: retiring a temporary
-- alias is the intended end state, so `base_app` gets DELETE here.

CREATE TABLE proof_topic_alias (
    alias      TEXT PRIMARY KEY,
    topic_id   TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_alias_slug_check CHECK (alias ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_alias_topic_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_alias_not_self CHECK (alias <> topic_id)
);

-- The read is "every alias of this topic" (list/show) and the reverse
-- single-alias lookup (resolve).
CREATE INDEX ix_proof_topic_alias_topic ON proof_topic_alias (topic_id);

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE proof_topic_alias TO base_app;
