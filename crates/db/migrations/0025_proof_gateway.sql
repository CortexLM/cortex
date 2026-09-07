-- Retain randomized v2 envelope signatures alongside the once-signed leaves.
-- Old decisions without one fail closed; never silently re-sign on delivery.
ALTER TABLE proof_atlas_decision ADD COLUMN publication_signature TEXT
    CHECK (publication_signature ~ '^[0-9a-f]{128}$');

-- Sticky, single-subnet configuration: restarting without v2 must not reopen v1.
CREATE TABLE gateway_proof_config (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    netuid INTEGER NOT NULL CHECK (netuid BETWEEN 0 AND 65535),
    anchor_block BIGINT NOT NULL CHECK (anchor_block >= 0),
    public_key BYTEA NOT NULL CHECK (octet_length(public_key) = 32)
);
CREATE TABLE gateway_proof_round (
    round BIGINT PRIMARY KEY CHECK (round >= 0),
    chain_epoch BIGINT NOT NULL CHECK (chain_epoch > 0),
    block_number BIGINT NOT NULL CHECK (block_number > 0),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    wire BYTEA NOT NULL CHECK (octet_length(wire) BETWEEN 1 AND 8388608),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE gateway_proof_seal (
    round BIGINT NOT NULL REFERENCES gateway_proof_round (round),
    epoch BIGINT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (epoch, revision),
    FOREIGN KEY (epoch, revision) REFERENCES epoch_bundle (epoch, revision)
);
GRANT SELECT, INSERT ON gateway_proof_config, gateway_proof_round, gateway_proof_seal TO base_app;

-- Also fence older gateway processes and the SECURITY DEFINER tip-upsert helper.
-- The shared transaction lock orders activation against in-flight legacy writes.
CREATE FUNCTION refuse_legacy_proof_weight() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF NEW.challenge_id = 'proof' THEN
        PERFORM pg_advisory_xact_lock(hashtext(current_schema()), 2525);
        IF EXISTS (SELECT 1 FROM gateway_proof_config) THEN
            RAISE EXCEPTION 'Proof v2 requires an authenticated round batch';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER refuse_legacy_proof_weight
BEFORE INSERT OR UPDATE ON raw_weight_snapshot
FOR EACH ROW EXECUTE FUNCTION refuse_legacy_proof_weight();

-- Only a seal committed atomically with round provenance can survive activation.
-- A late v1 seal from an older gateway cannot append after a newer v2 round.
CREATE FUNCTION require_proof_round_seal() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext(current_schema()), 2525);
    IF EXISTS (SELECT 1 FROM gateway_proof_config) AND NOT EXISTS (
        SELECT 1 FROM gateway_proof_seal s JOIN gateway_proof_round r ON r.round = s.round
        WHERE s.epoch = NEW.epoch AND s.revision = NEW.revision
          AND r.round = (SELECT MAX(round) FROM gateway_proof_round)
          AND r.chain_epoch = NEW.epoch AND r.block_number = NEW.block_number
    ) THEN
        RAISE EXCEPTION 'Proof v2 requires a current pinned round seal';
    END IF;
    RETURN NEW;
END;
$$;
CREATE CONSTRAINT TRIGGER require_proof_round_seal
AFTER INSERT ON epoch_bundle DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION require_proof_round_seal();
