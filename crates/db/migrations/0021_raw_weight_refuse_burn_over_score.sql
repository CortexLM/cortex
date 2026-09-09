-- Tip supersede must not let a ChallengeInternal burn (BUNDLE_SPEC §3.3.1
-- reason 6) replace a positive score for the same (challenge_id, epoch,
-- miner_hotkey). A restarted empty emitter used to POST that cover and the
-- next reseal rebuilt a uid-0 burn from the replacement leaves.
--
-- Identical-digest replay still returns NULL (HTTP 409). A refused burn
-- uses the same NULL / 409 path so the original paid row stays put.
-- Score-to-score and burn-to-score supersede are unchanged.

CREATE OR REPLACE FUNCTION upsert_raw_weight_tip(
    p_id uuid,
    p_challenge_id text,
    p_epoch bigint,
    p_miner_hotkey text,
    p_kind text,
    p_score bigint,
    p_absence_reason text,
    p_payload bytea,
    p_payload_digest bytea,
    p_signature bytea,
    p_nonce bytea
) RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    result_id uuid;
BEGIN
    INSERT INTO raw_weight_snapshot (
        id, challenge_id, epoch, miner_hotkey, kind, score, absence_reason,
        payload, payload_digest, signature, nonce
    ) VALUES (
        p_id, p_challenge_id, p_epoch, p_miner_hotkey, p_kind, p_score,
        p_absence_reason, p_payload, p_payload_digest, p_signature, p_nonce
    )
    ON CONFLICT (challenge_id, epoch, miner_hotkey) DO UPDATE SET
        id = EXCLUDED.id,
        kind = EXCLUDED.kind,
        score = EXCLUDED.score,
        absence_reason = EXCLUDED.absence_reason,
        payload = EXCLUDED.payload,
        payload_digest = EXCLUDED.payload_digest,
        signature = EXCLUDED.signature,
        nonce = EXCLUDED.nonce
    WHERE raw_weight_snapshot.payload_digest IS DISTINCT FROM EXCLUDED.payload_digest
      AND NOT (
          raw_weight_snapshot.kind = 'score'
          AND COALESCE(raw_weight_snapshot.score, 0) > 0
          AND EXCLUDED.kind = 'no_score'
          AND EXCLUDED.absence_reason = '6'
      )
    RETURNING id INTO result_id;

    RETURN result_id;
END;
$$;

REVOKE ALL ON FUNCTION upsert_raw_weight_tip(
    uuid, text, bigint, text, text, bigint, text, bytea, bytea, bytea, bytea
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION upsert_raw_weight_tip(
    uuid, text, bigint, text, text, bigint, text, bytea, bytea, bytea, bytea
) TO base_app;
