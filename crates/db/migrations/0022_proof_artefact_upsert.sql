-- proof_artefact is keyed by (topic_id, submission_id), not a journal.
-- A challenge restart that reused a pf_ id used to fail the metadata INSERT
-- (proof_artefact_pkey) after the zip on disk was already overwritten. Allow
-- the app role to replace metadata so the stored sha/path match the zip.
-- PK is unchanged. Journal tables stay INSERT-only.

GRANT UPDATE ON TABLE proof_artefact TO base_app;
