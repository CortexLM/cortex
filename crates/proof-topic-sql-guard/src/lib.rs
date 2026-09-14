//! What a topic migration may and may not touch.
//!
//! A topic's RLM install applies SQL into the **shared challenge database**.
//! Every topic lives in that one database under a `topic_id` discriminant, so
//! a migration that reached outside its own namespace would not be a bug in
//! one topic — it could rewrite another topic's rules, forge a promotion, or
//! drop the tables the whole subnet's scoring reads.
//!
//! This module is the gate in front of that. It is a **deny-list plus a
//! namespace check**, applied to every statement before the first one runs:
//!
//! - **Deny-listed objects**: every `proof_*` object this repository owns, the
//!   sqlx bookkeeping table, the roles, and the system catalogues. A
//!   migration may not name any of them, whatever the verb.
//! - **Deny-listed statements**: `DROP DATABASE` / `SCHEMA` / `ROLE` / `OWNED`
//!   / `EXTENSION`, privilege changes (`GRANT` / `REVOKE`), session and
//!   transaction control (`SET` / `RESET` / `BEGIN` / `COMMIT`), `COPY`,
//!   `VACUUM` / `CLUSTER` / `REINDEX`, `SECURITY DEFINER` functions, and
//!   server-side file access (`pg_read_file`, `lo_import`, …).
//! - **Namespace**: every table a statement creates, writes, or reads must be
//!   inside the topic's own namespace (`{topic_id}_*`, `topic_*`, or
//!   `{topic_id}.…`). Without this a topic could claim a generic name and
//!   collide with the next topic's install, or read a sibling topic's rows.
//!
//! # What this is not
//!
//! This is **not** a SQL parser. It is a conservative scanner over statement
//! text with strings, comments, and dollar-quoted bodies blanked first, so a
//! denied word inside a literal is not a false refusal and a denied statement
//! cannot be smuggled in by quoting. Because it is conservative it refuses on
//! *doubt*: an unrecognised shape is refused, and a legitimate migration that
//! the scanner does not recognise becomes a reviewed edit to this module
//! rather than a runtime surprise.
//!
//! # Literals are decoded, not read as written
//!
//! A function body may be written as a **string literal** rather than a
//! dollar-quoted one, and PostgreSQL does not execute the characters between
//! the quotes — it executes the *decoded* value. The scanner therefore
//! decodes every form PostgreSQL accepts before it scans a body, because a
//! denied statement spelled in an escape is still that statement:
//!
//! | Form | What PostgreSQL runs |
//! |------|----------------------|
//! | `'…''…'` | the doubled quote is one `'` |
//! | `E'\x44ELETE FROM …'` | `DELETE FROM …` (`\x44` is `D`) |
//! | `E'\104RANT …'` | `GRANT …` (octal, `\u`/`\U` likewise) |
//! | `E'…\'…'` | the backslash escapes the quote, so the body does not end there |
//! | `U&'\0044ELETE …'` | `DELETE …` (Unicode escapes, `UESCAPE 'c'` honoured) |
//! | `'DROP TABLE proof'`⏎`'_topic_version'` | one concatenated string |
//!
//! Two limitations are worth stating plainly, because a reader should not
//! assume more than this buys:
//!
//! 1. A table function (`generate_series(…)`) is skipped as a function call
//!    rather than treated as a table, and a comma-join (`FROM a, b`) is only
//!    checked for its first table. A migration could therefore read a shared
//!    non-`proof_*` table it named indirectly. It still cannot read a
//!    `proof_*` table (denied by name), which is what the scoring path owns.
//! 2. A dynamic statement built at runtime by a `plpgsql` body is scanned as
//!    text inside its dollar quotes and cannot be analysed. A migration that
//!    needs a function body is reviewed by hand.
//!
//! Both are why an install is an **operator** action against an
//! operator-published bundle, not a miner-facing path.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::collections::BTreeSet;

/// Why a migration statement was refused.
///
/// This crate owns its error rather than reusing the installer's, so the
/// guard is a self-contained rule: a caller can check a migration without
/// pulling in a database, a store, or a topic. The installer wraps this in
/// its own [`MigrationDenied`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("migration statement {ordinal} denied ({what}): {why}\n  statement: {statement}")]
pub struct MigrationDenied {
    /// 1-based position in the migration's statement list.
    pub ordinal: usize,
    /// The statement, shortened.
    pub statement: String,
    /// The token or construct that was refused.
    pub what: String,
    /// Why it is refused.
    pub why: String,
}

/// Prefix every object this repository owns carries.
///
/// A *deny* prefix: a topic migration naming any `proof_*` object is refused,
/// so a new `proof_*` table added by a later migration is protected without
/// editing any list here.
pub const OWNED_TABLE_PREFIX: &str = "proof_";

/// Objects this repository owns, by name, so a refusal can name the object.
///
/// The prefix check above is the enforcement; this list is what makes the
/// refusal *legible*, and a test asserts it covers every `proof_*` table the
/// migrations create.
pub const OWNED_TABLES: [&str; 9] = [
    "proof_topic_version",
    "proof_rule_version",
    "proof_checklist",
    "proof_lifecycle_event",
    "proof_baseline_measurement",
    "proof_artefact",
    "proof_promotion_event",
    "proof_topic_alias",
    "proof_topic_install",
];

/// Names a topic migration may never name, whatever the verb.
pub const DENIED_OBJECTS: [&str; 8] = [
    "_sqlx_migrations",
    "base_app",
    "pg_roles",
    "pg_authid",
    "information_schema",
    "pg_catalog",
    "pg_proc",
    "pg_shadow",
];

/// Statement verbs a topic migration may not use, with the reason.
///
/// Matched as whole words against the blanked statement, so `granted` is not
/// a `GRANT`. The first match refuses.
pub const DENIED_VERBS: [(&str, &str); 13] = [
    ("GRANT", "a topic migration may not change privileges"),
    ("REVOKE", "a topic migration may not change privileges"),
    (
        "SECURITY",
        "a topic migration may not create a SECURITY DEFINER function",
    ),
    (
        "COPY",
        "a topic migration may not use COPY (server-side file access)",
    ),
    ("VACUUM", "a topic migration may not VACUUM"),
    ("CLUSTER", "a topic migration may not CLUSTER"),
    ("REINDEX", "a topic migration may not REINDEX"),
    ("DISCARD", "a topic migration may not DISCARD session state"),
    ("LISTEN", "a topic migration may not LISTEN"),
    ("NOTIFY", "a topic migration may not NOTIFY"),
    ("RESET", "a topic migration may not change session settings"),
    (
        "BEGIN",
        "a topic migration may not manage its own transactions",
    ),
    (
        "COMMIT",
        "a topic migration may not manage its own transactions",
    ),
];

/// Statement verbs that only count when the statement **starts** with them.
///
/// `SET` is a legal word inside `UPDATE … SET …`, so it is refused only as a
/// statement head (`SET ROLE`, `SET search_path`).
pub const DENIED_HEADS: [(&str, &str); 1] =
    [("SET", "a topic migration may not change session settings")];

/// Object kinds a `DROP` may not name.
pub const DENIED_DROP_KINDS: [&str; 8] = [
    "DATABASE",
    "SCHEMA",
    "ROLE",
    "USER",
    "OWNED",
    "EXTENSION",
    "TABLESPACE",
    "SUBSCRIPTION",
];

/// Server-side functions that reach the database host's files.
pub const DENIED_FUNCTIONS: [&str; 8] = [
    "pg_read_file",
    "pg_read_binary_file",
    "pg_write_file",
    "pg_ls_dir",
    "pg_stat_file",
    "lo_import",
    "lo_export",
    "pg_execute_server_program",
];

/// Keywords after which a table name appears.
///
/// `TRUNCATE` and `DELETE` are here for the same reason as `FROM`: the object
/// they act on is the whole point of the statement, so it has to be checked.
/// A verb whose table is not in this list would let a topic name an object
/// outside its namespace and never be asked about it.
const TABLE_KEYWORDS: [&str; 8] = [
    "FROM", "JOIN", "INTO", "UPDATE", "TABLE", "INDEX", "TRUNCATE", "DELETE",
];

/// One statement, in both the forms the guard needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 1-based position in the migration.
    pub ordinal: usize,
    /// The statement verbatim — this is what gets executed.
    pub text: String,
    /// The same statement with strings, comments, and dollar-quoted bodies
    /// blanked — this is what gets scanned.
    pub blanked: String,
    /// Dollar-quoted **function bodies**, with their own literals blanked.
    ///
    /// A body is what actually runs, so it is scanned rather than trusted: a
    /// `DELETE FROM proof_rule_version` inside `$$ … $$` is refused exactly
    /// as it would be outside. Kept separately from [`Self::blanked`] so the
    /// refusal can say the denied token came from a body.
    pub bodies: String,
}

/// Split SQL into statements, keeping both the executable text and a blanked
/// copy for scanning.
///
/// Recognises `'…'` (with `''`), `"…"` (with `""`), `-- …`, `/* … */`
/// (nesting), and `$tag$ … $tag$`. A semicolon outside all of those ends a
/// statement. Anything unrecognised is copied verbatim, which can only make
/// the scanner *more* conservative.
#[must_use]
pub fn split_statements(sql: &str) -> Vec<Statement> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<Statement> = Vec::new();
    let mut text = String::new();
    let mut blanked = String::new();
    let mut bodies = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                // String literal: kept for execution, blanked for scanning.
                text.push(c);
                blanked.push(' ');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            text.push_str("''");
                            blanked.push_str("  ");
                            i += 2;
                            continue;
                        }
                        text.push('\'');
                        blanked.push(' ');
                        i += 1;
                        break;
                    }
                    text.push(chars[i]);
                    blanked.push(' ');
                    i += 1;
                }
            }
            '"' => {
                // Quoted identifier: it *names* an object, so both copies keep it.
                text.push('"');
                blanked.push('"');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' {
                        if chars.get(i + 1) == Some(&'"') {
                            text.push_str("\"\"");
                            blanked.push_str("\"\"");
                            i += 2;
                            continue;
                        }
                        text.push('"');
                        blanked.push('"');
                        i += 1;
                        break;
                    }
                    text.push(chars[i]);
                    blanked.push(chars[i]);
                    i += 1;
                }
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i = skip_block_comment(&chars, i);
            }
            '$' => {
                i = copy_dollar_quoted(&chars, i, &mut text, &mut blanked, &mut bodies);
            }
            ';' => {
                if !text.trim().is_empty() {
                    out.push(finish_statement(out.len() + 1, &text, &blanked, &bodies));
                }
                text.clear();
                blanked.clear();
                bodies.clear();
                i += 1;
            }
            _ => i = copy_chars(&chars, i, 1, &mut text, &mut blanked),
        }
    }
    if !text.trim().is_empty() {
        out.push(finish_statement(out.len() + 1, &text, &blanked, &bodies));
    }
    out
}

/// Build a [`Statement`], adding any **code-carrying string literal** to the
/// scanned bodies.
///
/// PostgreSQL accepts a function body as a string literal as well as a
/// dollar-quoted one:
///
/// ```sql
/// CREATE FUNCTION f() RETURNS void AS 'DELETE FROM proof_rule_version' LANGUAGE sql;
/// CREATE PROCEDURE p() AS E'\x44ROP TABLE proof_topic_version' LANGUAGE sql;
/// DO U&'\0044ELETE FROM proof_rule_version';
/// ```
///
/// The string-literal branch blanks that body in `blanked` (a literal is
/// normally *data*, not code), so without this step the body would be executed
/// without ever being scanned — a topic could install a function that reaches
/// a `proof_*` object by wrapping the statement in `AS '…'`. When the
/// statement is one whose literal is code, that literal is therefore decoded
/// and appended to [`Statement::bodies`], which [`check_statement`] scans
/// exactly as it scans a dollar-quoted body.
///
/// The detection is deliberately broad inside that class: any statement
/// mentioning `FUNCTION`/`PROCEDURE` and `AS`, or whose head is `DO`, has its
/// literals scanned. A false positive costs a migration nothing (its literals
/// are inert text that will not match a deny rule); a false negative would be
/// the hole above.
fn finish_statement(ordinal: usize, text: &str, blanked: &str, bodies: &str) -> Statement {
    let mut bodies = bodies.to_owned();
    if carries_code_in_literals(blanked) {
        for snippet in code_literals(text) {
            bodies.push(' ');
            bodies.push_str(&snippet);
        }
    }
    Statement {
        ordinal,
        text: text.trim().to_owned(),
        blanked: blanked.trim().to_owned(),
        bodies: bodies.trim().to_owned(),
    }
}

/// Whether a statement's string literals are **code** rather than data.
///
/// Three forms carry code in a literal:
///
/// - `CREATE [OR REPLACE] FUNCTION … AS <literal>`,
/// - `CREATE [OR REPLACE] PROCEDURE … AS <literal>` (same shape, different
///   object kind), and
/// - `DO [LANGUAGE lang] <literal>`, which is a literal body by definition.
///
/// The `DO` test is a **statement-head** test, not a word search, so
/// `INSERT … ON CONFLICT … DO UPDATE …` (whose literals are ordinary data)
/// is not dragged in.
fn carries_code_in_literals(blanked: &str) -> bool {
    if has_word(blanked, "AS") && (has_word(blanked, "FUNCTION") || has_word(blanked, "PROCEDURE"))
    {
        return true;
    }
    let head = blanked.trim_start();
    head.len() > 2
        && head[..2].eq_ignore_ascii_case("DO")
        && head[2..].starts_with(char::is_whitespace)
}

/// The literal code a statement carries, decoded as PostgreSQL would execute
/// it.
///
/// Every literal in a code-carrying statement is included, not only the one
/// after `AS`: PostgreSQL's grammar puts the body literal wherever the
/// operator wrote it, and a deny rule that only read the first one would be a
/// hole the operator could reach by reordering two clauses.
///
/// Each literal contributes up to two snippets — its **decoded** value (what
/// the server runs) and its **raw** spelling (what the operator wrote, when
/// the two differ) — plus the concatenation of any run of literals that
/// PostgreSQL would join into one string.
fn code_literals(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let literals = scan_literals(&chars);
    let mut out: Vec<String> = Vec::new();
    let mut joined: Option<String> = None;
    for (i, literal) in literals.iter().enumerate() {
        let concatenated_with_previous = i > 0
            && i.checked_sub(1).is_some_and(|p| {
                let prev = &literals[p];
                let gap: String = chars
                    .get(prev.end..literal.start)
                    .unwrap_or_default()
                    .iter()
                    .collect();
                !gap.is_empty() && gap.chars().all(char::is_whitespace) && gap.contains('\n')
            });
        if concatenated_with_previous {
            let base = joined
                .take()
                .unwrap_or_else(|| literals[i - 1].decoded.clone());
            joined = Some(base + &literal.decoded);
        } else if let Some(done) = joined.take() {
            push_snippet(&mut out, &done);
        }
        push_snippet(&mut out, &literal.decoded);
        if literal.raw != literal.decoded {
            push_snippet(&mut out, &literal.raw);
        }
    }
    if let Some(done) = joined {
        push_snippet(&mut out, &done);
    }
    out
}

/// Append a snippet that carries something to scan.
fn push_snippet(out: &mut Vec<String>, snippet: &str) {
    if !snippet.trim().is_empty() {
        out.push(snippet.to_owned());
    }
}

/// The string-literal form a quote opens, which decides how its content is
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    /// `'…'` — `''` is a quote; a backslash is an ordinary character.
    Standard,
    /// `E'…'` — `''` is a quote, and a backslash escapes what follows
    /// (`\x44`, `\104`, `\u0044`, `\'`, `\\`, …).
    Escape,
    /// `U&'…'` — `''` is a quote, and the escape character (a backslash by
    /// default, or whatever a following `UESCAPE 'c'` names) introduces
    /// `XXXX` / `+XXXXXX` code points.
    Unicode,
}

/// One string literal found in a statement's text.
struct Literal {
    /// Index of the opening quote.
    start: usize,
    /// Index just past the literal and any `UESCAPE` clause.
    end: usize,
    /// The content as written between the quotes.
    raw: String,
    /// The content as PostgreSQL would execute it: escapes decoded.
    decoded: String,
}

/// Every string literal in `chars`, in order.
///
/// A quote whose form the scanner does not recognise is still read as a
/// standard literal, so nothing between quotes is ever left unblanked in the
/// statement scan.
fn scan_literals(chars: &[char]) -> Vec<Literal> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let Some((quote, kind)) = literal_start(chars, i) else {
            i += 1;
            continue;
        };
        let literal = read_literal(chars, quote, kind);
        i = literal.end.max(quote + 1);
        out.push(literal);
    }
    out
}

/// Where a string literal starts, and which form it is.
///
/// The prefix is only a prefix when it is a whole word: `E'…'` is an escape
/// string, `xE'…'` is not.
fn literal_start(chars: &[char], i: usize) -> Option<(usize, LiteralKind)> {
    let c = chars[i];
    if c == '\'' {
        return Some((i, LiteralKind::Standard));
    }
    if !is_word_char(c) || (i > 0 && is_word_char(chars[i - 1])) {
        return None;
    }
    match c {
        'E' | 'e' if chars.get(i + 1) == Some(&'\'') => Some((i + 1, LiteralKind::Escape)),
        'U' | 'u' if chars.get(i + 1) == Some(&'&') && chars.get(i + 2) == Some(&'\'') => {
            Some((i + 2, LiteralKind::Unicode))
        }
        // Bit and hex strings (`B'1010'`, `X'1f'`) are numbers, not code:
        // reading them as standard strings keeps their digits scanned like
        // any other literal, which is all a scanner needs from them.
        'B' | 'b' | 'X' | 'x' if chars.get(i + 1) == Some(&'\'') => {
            Some((i + 1, LiteralKind::Standard))
        }
        _ => None,
    }
}

/// A word character, for the prefix boundary test.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Read one literal: its extent, its escape character, and its decoded value.
fn read_literal(chars: &[char], quote: usize, kind: LiteralKind) -> Literal {
    let (raw, after) = literal_extent(chars, quote, kind);
    // A `U&'…'` string may rename its escape character with a following
    // `UESCAPE 'c'` clause. The clause is read **after** the extent, because
    // the extent does not depend on it: a `U&` string ends at the first quote
    // that is not doubled whatever the escape character is.
    let (escape, end) = match kind {
        LiteralKind::Unicode => uescape_clause(chars, after),
        _ => ('\\', after),
    };
    Literal {
        start: quote,
        end,
        decoded: decode_literal(&raw, kind, escape),
        raw,
    }
}

/// The raw content between the quotes, and the index just past the closing
/// quote.
///
/// `''` is always one quote. In an `E'…'` string a backslash escapes the
/// character after it, so `\'` does **not** close the literal — the body runs
/// on, exactly as PostgreSQL reads it.
fn literal_extent(chars: &[char], quote: usize, kind: LiteralKind) -> (String, usize) {
    let mut raw = String::new();
    let mut i = quote + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                raw.push_str("''");
                i += 2;
                continue;
            }
            return (raw, i + 1);
        }
        if kind == LiteralKind::Escape && c == '\\' {
            raw.push('\\');
            i += 1;
            if let Some(next) = chars.get(i) {
                raw.push(*next);
                i += 1;
            }
            continue;
        }
        raw.push(c);
        i += 1;
    }
    // Unterminated: the server refuses the statement, so nothing runs. The
    // rest of the text is kept as the literal's content, which can only make
    // the scan read *more* as code.
    (raw, chars.len())
}

/// A `U&'…'` string's escape character and the index just past the clause.
///
/// `UESCAPE 'c'` names the character that introduces a code point; the default
/// is a backslash. A clause PostgreSQL would refuse (a hex digit, `+`, a
/// quote, or a malformed spelling) leaves the default in place rather than
/// inventing a second reading: the statement does not run, so nothing hides.
fn uescape_clause(chars: &[char], after: usize) -> (char, usize) {
    let default = ('\\', after);
    let mut i = after;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    let word: String = chars.iter().skip(i).take(7).collect();
    if !word.eq_ignore_ascii_case("UESCAPE") {
        return default;
    }
    let mut j = i + 7;
    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
        j += 1;
    }
    if chars.get(j) != Some(&'\'') || chars.get(j + 2) != Some(&'\'') {
        return default;
    }
    let Some(c) = chars.get(j + 1).copied() else {
        return default;
    };
    if c.is_ascii_hexdigit() || matches!(c, '+' | '\'' | '"') {
        return default;
    }
    (c, j + 3)
}

/// Decode a literal's content the way PostgreSQL reads it.
///
/// The result is the value the server would execute, so a body that spells a
/// denied statement in escapes is scanned in the form that actually runs. A
/// sequence PostgreSQL would refuse is kept in its written form, which is the
/// conservative read: the statement does not run, and the spelling is scanned
/// too.
fn decode_literal(raw: &str, kind: LiteralKind, escape: char) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut bytes: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' && chars.get(i + 1) == Some(&'\'') {
            bytes.push(b'\'');
            i += 2;
            continue;
        }
        match kind {
            LiteralKind::Escape if c == '\\' => i = decode_escape(&chars, i, &mut bytes),
            LiteralKind::Unicode if c == escape => {
                i = decode_unicode_escape(&chars, i, escape, &mut bytes);
            }
            _ => {
                push_char(&mut bytes, c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Decode one `E'…'` escape sequence, returning the index just past it.
///
/// `\x` takes one or two hex digits, `\o`/`\oo`/`\ooo` one to three octal
/// digits, `\u` four and `\U` eight hex digits, and any other character after
/// the backslash stands for itself (`\'`, `\\`, `\n`, `\t`, …). A sequence
/// with too few digits is one PostgreSQL refuses; it is kept as written.
fn decode_escape(chars: &[char], start: usize, out: &mut Vec<u8>) -> usize {
    let mut i = start + 1;
    let Some(c) = chars.get(i).copied() else {
        out.push(b'\\');
        return i;
    };
    i += 1;
    match c {
        'b' => out.push(0x08),
        'f' => out.push(0x0c),
        'n' => out.push(b'\n'),
        'r' => out.push(b'\r'),
        't' => out.push(b'\t'),
        '0'..='7' => {
            let mut value = c.to_digit(8).unwrap_or(0);
            for _ in 0..2 {
                match chars.get(i).and_then(|d| d.to_digit(8)) {
                    Some(d) => {
                        value = value * 8 + d;
                        i += 1;
                    }
                    None => break,
                }
            }
            out.push(u8::try_from(value).unwrap_or(0));
        }
        'x' => match hex_at(chars, i, 1, 2) {
            Some((value, next)) => {
                out.push(u8::try_from(value).unwrap_or(0));
                i = next;
            }
            None => out.extend_from_slice(b"\\x"),
        },
        'u' => match hex_at(chars, i, 4, 4) {
            Some((value, next)) => {
                push_code_point(out, value, chars, start, next);
                i = next;
            }
            None => out.extend_from_slice(b"\\u"),
        },
        'U' => match hex_at(chars, i, 8, 8) {
            Some((value, next)) => {
                push_code_point(out, value, chars, start, next);
                i = next;
            }
            None => out.extend_from_slice(b"\\U"),
        },
        other => push_char(out, other),
    }
    i
}

/// Decode one `U&'…'` escape sequence, returning the index just past it.
///
/// `<esc>XXXX` (four hex digits) and `<esc>+XXXXXX` (six) are code points, and
/// a doubled escape character is one literal escape character. Anything else
/// is a sequence PostgreSQL refuses, kept as written.
fn decode_unicode_escape(chars: &[char], start: usize, escape: char, out: &mut Vec<u8>) -> usize {
    let i = start + 1;
    let Some(c) = chars.get(i).copied() else {
        push_char(out, escape);
        return i;
    };
    if c == escape {
        push_char(out, escape);
        return i + 1;
    }
    if c == '+' {
        if let Some((value, next)) = hex_at(chars, i + 1, 6, 6) {
            push_code_point(out, value, chars, start, next);
            return next;
        }
    } else if let Some((value, next)) = hex_at(chars, i, 4, 4) {
        push_code_point(out, value, chars, start, next);
        return next;
    }
    push_char(out, escape);
    i
}

/// Push a decoded code point as UTF-8, keeping the written spelling when the
/// code point is not one PostgreSQL would accept (a surrogate, or out of
/// range): that statement does not run, and the spelling is scanned as well.
fn push_code_point(out: &mut Vec<u8>, value: u32, chars: &[char], start: usize, to: usize) {
    match char::from_u32(value) {
        Some(c) => push_char(out, c),
        None => {
            for c in chars.iter().skip(start).take(to.saturating_sub(start)) {
                push_char(out, *c);
            }
        }
    }
}

/// `min..=max` hex digits starting at `i`, as a number and the index past it.
fn hex_at(chars: &[char], i: usize, min: usize, max: usize) -> Option<(u32, usize)> {
    let mut value = 0u32;
    let mut taken = 0usize;
    while taken < max {
        let Some(d) = chars.get(i + taken).and_then(|c| c.to_digit(16)) else {
            break;
        };
        value = value * 16 + d;
        taken += 1;
    }
    if taken < min {
        return None;
    }
    Some((value, i + taken))
}

/// Push one character as UTF-8 bytes.
fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Skip a `/* … */` comment (PostgreSQL allows nesting), returning the index
/// just past its close.
fn skip_block_comment(chars: &[char], start: usize) -> usize {
    let mut depth = 1usize;
    let mut i = start + 2;
    while i < chars.len() && depth > 0 {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            depth += 1;
            i += 2;
        } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    i
}

/// Copy a `$tag$ … $tag$` body: kept verbatim in `text`, blanked in
/// `blanked`, and copied into `bodies` with its own literals blanked so the
/// body is *scanned* rather than trusted.
fn copy_dollar_quoted(
    chars: &[char],
    start: usize,
    text: &mut String,
    blanked: &mut String,
    bodies: &mut String,
) -> usize {
    let Some(tag) = dollar_tag(&chars[start..]) else {
        return copy_chars(chars, start, 1, text, blanked);
    };
    let open = format!("${tag}$");
    let mut i = copy_through(chars, start, open.chars().count(), text, blanked);
    let rest: String = chars[i..].iter().collect();
    let (body, consumed) = match rest.find(&open) {
        Some(pos) => {
            let take = pos + open.len();
            (rest[..take].to_owned(), rest[..take].chars().count())
        }
        None => (rest.clone(), rest.chars().count()),
    };
    bodies.push_str(&blank_literals(&body));
    bodies.push(' ');
    i = copy_through(chars, i, consumed, text, blanked);
    i
}

/// `text` with single-quoted literals replaced by spaces.
///
/// The body's **statements** are what get scanned, so its literals are
/// blanked the same way the outer statement's are: a body storing the string
/// `'DROP DATABASE'` is data, not a statement.
fn blank_literals(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            out.push(' ');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push_str("  ");
                        i += 2;
                        continue;
                    }
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(' ');
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Copy `n` characters through `text` verbatim and `blanked` as spaces.
fn copy_through(
    chars: &[char],
    start: usize,
    n: usize,
    text: &mut String,
    blanked: &mut String,
) -> usize {
    let mut i = start;
    for _ in 0..n {
        if let Some(c) = chars.get(i) {
            text.push(*c);
            blanked.push(' ');
            i += 1;
        }
    }
    i
}

/// The blanked form of every statement, for callers that only scan.
#[must_use]
pub fn blank_statements(sql: &str) -> Vec<String> {
    split_statements(sql)
        .into_iter()
        .map(|s| s.blanked)
        .collect()
}

/// The `$tag$` opening a dollar-quoted string, if `chars` starts with one.
fn dollar_tag(chars: &[char]) -> Option<String> {
    if chars.first() != Some(&'$') {
        return None;
    }
    let mut tag = String::new();
    for c in chars.iter().skip(1) {
        if *c == '$' {
            return Some(tag);
        }
        if c.is_alphanumeric() || *c == '_' {
            tag.push(*c);
        } else {
            return None;
        }
    }
    None
}

/// Word-boundary, case-insensitive search on blanked text.
#[must_use]
pub fn has_word(haystack: &str, needle: &str) -> bool {
    let up = haystack.to_ascii_uppercase();
    let want = needle.to_ascii_uppercase();
    let mut from = 0usize;
    while let Some(pos) = up[from..].find(&want) {
        let start = from + pos;
        let end = start + want.len();
        let before_ok = start == 0
            || !up[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_ok = end >= up.len()
            || !up[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// Identifier-shaped tokens, lower-cased, dots kept (`schema.table`).
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(cur.trim_matches('.').to_ascii_lowercase());
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push(cur.trim_matches('.').to_ascii_lowercase());
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Object names following any of [`TABLE_KEYWORDS`], skipping `IF NOT EXISTS`
/// / `OR REPLACE`, and skipping function calls (`name(`).
#[must_use]
pub fn referenced_objects(statement: &str) -> Vec<String> {
    let toks = tokens(statement);
    let mut out = BTreeSet::new();
    for (i, tok) in toks.iter().enumerate() {
        if !TABLE_KEYWORDS.iter().any(|k| tok.eq_ignore_ascii_case(k)) {
            continue;
        }
        let mut j = i + 1;
        while j < toks.len()
            && [
                "if",
                "not",
                "exists",
                "or",
                "replace",
                "only",
                "into",
                "unique",
                "concurrently",
            ]
            .iter()
            .any(|s| toks[j].eq_ignore_ascii_case(s))
        {
            j += 1;
        }
        let Some(name) = toks.get(j) else { continue };
        // A table function (`generate_series(…)`) is not a table. The token
        // stream drops parentheses, so check the source text instead.
        if is_function_call(statement, name) {
            continue;
        }
        if is_sql_keyword(name) {
            continue;
        }
        out.insert(name.clone());
    }
    out.into_iter().collect()
}

/// Whether `name` appears as a **function call** (`name(`, no space) in
/// `statement`.
///
/// The no-space rule is what separates `generate_series(1, 10)` from
/// `CREATE TABLE x (id TEXT)`: PostgreSQL requires a function call to have no
/// whitespace before its parenthesis, while a column list always has one. A
/// whitespace-tolerant check here would classify every `CREATE TABLE … (…)`
/// as a function call and let every unscoped table name through.
fn is_function_call(statement: &str, name: &str) -> bool {
    let lower = statement.to_ascii_lowercase();
    let want = name.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find(&want) {
        let start = from + pos;
        let end = start + want.len();
        if lower[end..].starts_with('(') {
            return true;
        }
        from = end;
    }
    false
}

/// Keywords that are never a table name in the position the scanner reads.
///
/// `FROM` is here because `DELETE` and `TRUNCATE` are keywords a table name
/// follows: in `DELETE FROM x`, the token after `DELETE` is `FROM`, and the
/// table is the token after *that*. Skipping a keyword here is what lets the
/// scan continue to the real name instead of refusing the statement for
/// naming `from`.
fn is_sql_keyword(tok: &str) -> bool {
    matches!(
        tok,
        "select"
            | "from"
            | "where"
            | "values"
            | "set"
            | "and"
            | "or"
            | "not"
            | "null"
            | "default"
            | "lateral"
            | "unnest"
            | "true"
            | "false"
    )
}

/// Copy `chars[i..i + n]` through to both buffers, returning the new index.
fn copy_chars(
    chars: &[char],
    i: usize,
    n: usize,
    text: &mut String,
    blanked: &mut String,
) -> usize {
    let mut idx = i;
    for _ in 0..n {
        if let Some(c) = chars.get(idx) {
            text.push(*c);
            blanked.push(*c);
            idx += 1;
        }
    }
    idx
}

/// Whether `name` is inside `topic_id`'s namespace.
///
/// Two spellings are the topic's, and only two:
///
/// - a `{topic_id}`-qualified name (`tb4.scores`, `tb4.runs`), or
/// - a bare `{topic_id}_`-prefixed name (`tb4_scores`).
///
/// # Why there is no generic `topic_` allowance
///
/// An earlier revision also accepted any `topic_*` name, on the theory that a
/// shared prefix was a convenient place for a topic's scratch tables. It is
/// not: every topic shares one database, so `topic_scores` is *one* table that
/// every topic's install can reach. A migration approved for topic A could
/// then write, truncate, or redefine the table topic B created — the prefix
/// would be a naming convention, not an isolation boundary, and this guard's
/// whole job is to be the boundary.
///
/// Namespacing by the topic's own id is what makes "a topic may only touch its
/// own objects" enforceable by string comparison. A topic that wants a shared
/// table needs an operator-owned object created by a migration in
/// `crates/db/migrations/`, which is exactly the review this guard exists to
/// force.
#[must_use]
pub fn is_topic_scoped(name: &str, topic_id: &str) -> bool {
    let n = name.trim().trim_matches('"').to_ascii_lowercase();
    if n.is_empty() {
        return false;
    }
    let topic = topic_id.trim().to_ascii_lowercase();
    let (schema, bare) = match n.split_once('.') {
        Some((s, b)) => (Some(s), b),
        None => (None, n.as_str()),
    };
    if schema == Some(topic.as_str()) {
        return true;
    }
    bare.starts_with(&format!("{topic}_"))
}

/// Check a statement's text against every deny rule.
///
/// `where_` names the part of the statement being checked (`statement` or
/// `function body`), so a refusal from inside a dollar-quoted body says so.
fn check_text(
    statement: &Statement,
    text: &str,
    topic_id: &str,
    where_: &str,
) -> Result<(), MigrationDenied> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let deny = |what: &str, why: &str| MigrationDenied {
        ordinal: statement.ordinal,
        statement: truncate(&statement.blanked, 160),
        what: what.to_owned(),
        why: format!("{why} ({where_})"),
    };

    for (verb, why) in DENIED_VERBS {
        if has_word(text, verb) {
            return Err(deny(verb, why));
        }
    }
    let head = text.trim_start();
    for (verb, why) in DENIED_HEADS {
        if head.len() >= verb.len() && head[..verb.len()].eq_ignore_ascii_case(verb) {
            let rest = &head[verb.len()..];
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Err(deny(verb, why));
            }
        }
    }

    // `DROP` kinds are checked before the token scan so the refusal names the
    // construct the operator wrote (`DROP ROLE`) rather than a role name.
    if has_word(text, "DROP") {
        for kind in DENIED_DROP_KINDS {
            if has_word(text, kind) {
                return Err(deny(
                    &format!("DROP {kind}"),
                    "a topic migration may not drop a database, schema, role, or extension",
                ));
            }
        }
    }

    for func in DENIED_FUNCTIONS {
        if has_word(text, func) {
            return Err(deny(
                func,
                "server-side file access is not a topic migration",
            ));
        }
    }

    for token in tokens(text) {
        let base = token.rsplit('.').next().unwrap_or(&token);
        if DENIED_OBJECTS
            .iter()
            .any(|d| base.eq_ignore_ascii_case(d) || token.eq_ignore_ascii_case(d))
        {
            return Err(deny(
                &token,
                "the shared database's own objects are not a topic's to touch",
            ));
        }
        if base.starts_with(OWNED_TABLE_PREFIX) {
            return Err(deny(
                &token,
                "every proof_* object belongs to this repository's scoring path; a topic \
                 migration may not read, write, or redefine one",
            ));
        }
    }

    for name in referenced_objects(text) {
        if !is_topic_scoped(&name, topic_id) {
            return Err(deny(
                &name,
                &format!(
                    "a topic migration may only touch objects named {topic_id}_*, topic_*, or \
                     {topic_id}.*; an unscoped name would collide with — or read — another \
                     topic's install"
                ),
            ));
        }
    }
    Ok(())
}

/// Refuse a statement that reaches outside the topic's namespace.
///
/// Both the statement itself and every dollar-quoted **function body** it
/// carries are checked: a body is what actually runs, so a
/// `DELETE FROM proof_rule_version` inside `$$ … $$` is refused exactly as it
/// would be outside one.
///
/// # Errors
///
/// [`MigrationDenied`] naming the statement ordinal, the
/// offending token, and why.
pub fn check_statement(statement: &Statement, topic_id: &str) -> Result<(), MigrationDenied> {
    check_text(statement, &statement.blanked, topic_id, "statement")?;
    check_text(statement, &statement.bodies, topic_id, "function body")
}

/// Check every statement of one migration.
///
/// # Errors
///
/// The first [`MigrationDenied`].
pub fn check_migration(sql: &str, topic_id: &str) -> Result<Vec<Statement>, MigrationDenied> {
    let statements = split_statements(sql);
    if statements.is_empty() {
        return Err(MigrationDenied {
            ordinal: 0,
            statement: String::new(),
            what: "empty migration".to_owned(),
            why: "a migration with no statements installs nothing; remove it from the bundle"
                .to_owned(),
        });
    }
    for statement in &statements {
        check_statement(statement, topic_id)?;
    }
    Ok(statements)
}

/// Shorten a statement for an error message.
fn truncate(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    }
}
