-- Frozen submission digest is the retry identity.
--
-- 0031 on checkpoint/proof-production-readiness-20260907 already creates
-- `proof_submission_id_seq`. Do not replay or edit 0031 on an already-migrated
-- env: sqlx records it as applied. A pre-squash 0031 that lacked the sequence
-- needs a manual `CREATE SEQUENCE IF NOT EXISTS` repair, not a 0031 rewrite.
CREATE UNIQUE INDEX proof_submission_freeze
    ON proof_submission ((document->>'submission_digest'))
    WHERE coalesce(document->>'submission_digest', '') <> '';
