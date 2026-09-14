-- Proof topic install: what a topic's RLM install produced, and the topic
-- APIs it exposes.
--
-- A topic's *identity* stays where it has always been: the operator-signed
-- document in `proof_topic_version` (migration 0020), plus the RLM-authored
-- rule versions in `proof_rule_version`. Nothing here restates a binding
-- that already lives in a signed document, and nothing here is a second
-- topic table.
--
-- What was missing is where the two things an **RLM install** produces go:
--
-- 1. `proof_topic_install` — the record of one `proof-admin topic install`
--    run against a topic: which bundle digest was applied, whether the setup
--    reached a green state, the rules version it landed, the migrations it
--    applied, and the executor binding it resolved (handler, runner, custom
--    id, pack pin, submission-format digest, VMs per submission). It is a
--    *journal*, so an operator can see whether a topic was installed at all,
--    from which bundle, and — after a failure — exactly what was applied
--    before it stopped. One row per attempt; the newest row for a topic is
--    its current install state, the way the newest `proof_topic_version` row
--    is the current document.
-- 2. `proof_topic_api` — the dynamic routes a topic registered for itself
--    through its bundle's `rlm.apis` section. The control plane has no
--    compile-time route table a topic could extend, so an install *records*
--    the routes a topic claims and the challenge mux reads this table instead
--    of any compiled-in list. A topic can only claim paths **relative to its
--    own prefix**, which the CHECK below makes structural rather than a
--    convention a future edit could forget: the stored path has no leading
--    slash, so the resolver owns the prefix.
--
-- Both tables are topic-scoped by a `topic_id` discriminant in the one
-- shared challenge DB, exactly like every other `proof_*` table. There is no
-- per-topic schema.
--
-- Ownership: neither table may hold a binding that contradicts the signed
-- document. `bundle_digest` is the sha256 of the canonical bundle the
-- operator validated, so a later install of a *different* bundle is visible
-- as a different digest rather than as a silent overwrite. `rules_version`
-- names the `proof_rule_version` row the install landed, so the gate a topic
-- was installed under is replayable.
--
-- Nothing here weakens a gate: both tables are INSERT + SELECT for `base_app`,
-- with no UPDATE and no DELETE, because a journal that could be edited in
-- place would not be a journal. A re-install appends a row, and idempotency
-- comes from reading the journal (which migrations a topic already applied),
-- not from rewriting history.

CREATE TABLE proof_topic_install (
    id            BIGSERIAL PRIMARY KEY,
    topic_id      TEXT NOT NULL,
    -- `sha256:<64 lowercase hex>` over the canonical bundle. Never invented:
    -- the CLI computes it from the bundle it validated.
    bundle_digest TEXT NOT NULL,
    -- Install target (`staging` | `metal`), for the operator's audit trail.
    environment   TEXT NOT NULL,
    -- Where the install got to. `pending` is written before any RLM call, so
    -- a crash mid-install leaves evidence rather than silence; `applied` is
    -- written only once every step succeeded.
    state         TEXT NOT NULL,
    -- The rule version the install landed, once the RLM wrote one.
    rules_version INTEGER,
    -- Rule ids installed, for a readable audit line.
    rule_ids      JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- Names of the migrations this install applied, in order. The journal's
    -- union over a topic is what makes a re-install skip work already done.
    migrations    JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- The executor binding this install resolved: handler family, runner id,
    -- custom id, pack pin and directory, submission-format and scoring
    -- digests, and the VMs-per-submission pin. Recorded so an audit can see
    -- what a topic was installed *with*, without a second registry.
    binding       JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Why the install stopped, when it did. Operator-readable, never a secret.
    detail        TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proof_topic_install_id_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT proof_topic_install_digest_check CHECK (bundle_digest ~ '^sha256:[0-9a-f]{64}$'),
    CONSTRAINT proof_topic_install_env_check CHECK (environment IN ('staging', 'metal')),
    -- The states an install moves through. `pending` → `applied` is the happy
    -- path; `failed` is where a refusal lands, with `detail` saying why.
    CONSTRAINT proof_topic_install_state_check CHECK (state IN ('pending', 'applied', 'failed')),
    CONSTRAINT proof_topic_install_rules_pos CHECK (rules_version IS NULL OR rules_version >= 1)
);

-- The read is "the newest install for this topic" and, for the operator
-- listing, "newest first across topics".
CREATE INDEX ix_proof_topic_install_topic ON proof_topic_install (topic_id, id DESC);

CREATE TABLE proof_topic_api (
    topic_id   TEXT NOT NULL,
    -- Path the topic claims, **relative to its own prefix**. Stored without a
    -- leading slash so a row cannot carry an absolute path that escapes the
    -- topic's namespace; the resolver builds `{prefix}{path}`.
    path       TEXT NOT NULL,
    -- HTTP method the route answers. `*` is any method.
    method     TEXT NOT NULL,
    -- What the route does, in the topic's own words. Never interpreted.
    summary    TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, method, path),
    CONSTRAINT proof_topic_api_topic_check CHECK (topic_id ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    -- A relative path of plain segments: no leading `/`, no `..`, no empty
    -- segment, no query, no backslash. This is what makes "a topic can only
    -- claim routes under its own prefix" structural.
    CONSTRAINT proof_topic_api_path_check CHECK (
        path <> ''
        AND path !~ '^/'
        AND path !~ '//'
        AND path !~ '/$'
        AND path !~ '\.\.'
        AND path !~ '[\\?#]'
        AND path ~ '^[A-Za-z0-9._~-]+(/[A-Za-z0-9._~-]+)*$'
    ),
    CONSTRAINT proof_topic_api_method_check CHECK (
        method IN ('GET', 'POST', 'PUT', 'PATCH', 'DELETE', '*')
    )
);

-- The read is "every route this topic exposes" (the mux) and "every route
-- under this prefix" (resolution).
CREATE INDEX ix_proof_topic_api_topic ON proof_topic_api (topic_id, path);

-- Append-only for the application role: an install journal and a route claim
-- are facts about what happened, not state to be edited in place. A topic
-- that changes its routes re-installs, which appends.
GRANT SELECT, INSERT ON TABLE proof_topic_install TO base_app;
GRANT USAGE, SELECT ON SEQUENCE proof_topic_install_id_seq TO base_app;
GRANT SELECT, INSERT ON TABLE proof_topic_api TO base_app;
