-- proof_checklist is keyed by submission_digest (frozen artefact identity),
-- not a journal. A miner resubmit of the same artefact after a false
-- anti-cheat reject used to fail the metadata INSERT (proof_checklist_pkey)
-- and surface as 503. Allow the app role to replace the latest inspection
-- (topic_id / rules_version / green / failed_ids / document). created_at
-- stays the original row. PK is unchanged. True journals stay INSERT-only.

GRANT UPDATE ON TABLE proof_checklist TO base_app;
