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
-- A canonical slug is never shadowed. If an alias equals some *other*
-- published topic's id, resolving that id as an alias would hand back a
-- different topic's signed document. The store refuses to write such a row
-- and refuses to resolve one, and the trigger below closes the same hole for
-- a writer that goes straight to SQL. Both directions are needed: the trigger
-- fires when the alias is written, and again when a topic is published under
-- a name that an existing alias already claims.
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

-- `topic_id` alone is not unique in `proof_topic_version` (it is keyed by
-- `(topic_id, version)`), so the shadow guard cannot be a UNIQUE constraint.
-- It is a trigger instead, checked in both directions: a published topic may
-- not be claimed as an alias, and an alias may not be published as a topic.
CREATE OR REPLACE FUNCTION proof_topic_alias_no_shadow() RETURNS trigger AS $$
BEGIN
    IF TG_TABLE_NAME = 'proof_topic_alias' THEN
        IF EXISTS (SELECT 1 FROM proof_topic_version WHERE topic_id = NEW.alias) THEN
            RAISE EXCEPTION 'alias % is already a published topic id', NEW.alias
                USING ERRCODE = 'check_violation';
        END IF;
    ELSE
        IF EXISTS (SELECT 1 FROM proof_topic_alias WHERE alias = NEW.topic_id) THEN
            RAISE EXCEPTION 'topic % is already claimed as an alias', NEW.topic_id
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER proof_topic_alias_no_shadow
    BEFORE INSERT OR UPDATE ON proof_topic_alias
    FOR EACH ROW EXECUTE FUNCTION proof_topic_alias_no_shadow();

CREATE TRIGGER proof_topic_version_no_shadow
    BEFORE INSERT ON proof_topic_version
    FOR EACH ROW EXECUTE FUNCTION proof_topic_alias_no_shadow();

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE proof_topic_alias TO base_app;
