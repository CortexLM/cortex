//! Fail if a pull-request body is missing the `.rules/` attestation.
//!
//! [`REQUIRED`] is the single source of truth: `rules-check` asserts the same
//! phrases exist in `.github/PULL_REQUEST_TEMPLATE.md`, so a filled-in
//! template always satisfies this gate.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Checkbox phrases a PR body must carry, checked (`[x]`).
///
/// Compared after normalisation (backticks dropped, whitespace collapsed,
/// lowercased) so template formatting can change without breaking the gate.
pub const REQUIRED: &[&str] = &[
    "I read all of .rules/ before opening this PR and before marking it ready",
    "AGENTS.md, README.md and .rules/ are accurate for this change, or N/A with a reason below",
    "Local pre-prod gates in .rules/20-pre-prod-local.md all passed",
    "Version bumped per .rules/50-versioning.md",
];

/// Run the PR attestation gate.
///
/// # Errors
///
/// Returns an error when the body cannot be read, or when it is missing a
/// required checked attestation and `draft` is false.
pub fn run(body_file: &Path, draft: bool) -> Result<(), String> {
    let body = read_body(body_file)?;
    let failures = missing(&body);

    if failures.is_empty() {
        println!("pr-check: OK ({} attestations checked)", REQUIRED.len());
        return Ok(());
    }

    let report = failures
        .iter()
        .map(|f| format!("  - {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    if draft {
        println!(
            "pr-check: draft PR, {} attestation(s) still open:\n{report}\n\
             Tick every box before marking this PR ready for review.",
            failures.len()
        );
        return Ok(());
    }

    Err(format!(
        "pr-check failed ({}):\n{report}\n\n\
         Copy the checklist from .github/PULL_REQUEST_TEMPLATE.md into the PR body and tick \
         every box after actually reading .rules/ and running the gates.",
        failures.len()
    ))
}

fn read_body(body_file: &Path) -> Result<String, String> {
    if body_file == Path::new("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("read PR body from stdin: {e}"))?;
        return Ok(buf);
    }
    fs::read_to_string(body_file).map_err(|e| format!("read {}: {e}", body_file.display()))
}

fn missing(body: &str) -> Vec<String> {
    let normalized = normalize(body);
    let mut out = Vec::new();
    if normalized.trim().is_empty() {
        out.push(String::from("PR body is empty"));
        return out;
    }
    for phrase in REQUIRED {
        let want = normalize(phrase);
        if normalized.contains(&format!("[x] {want}")) {
            continue;
        }
        if normalized.contains(&format!("[ ] {want}")) {
            out.push(format!("attestation left unchecked: {phrase}"));
        } else {
            out.push(format!("attestation missing from the body: {phrase}"));
        }
    }
    out
}

/// Drop markdown emphasis noise and collapse whitespace, then lowercase.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        if ch == '`' || ch == '*' || ch == '_' {
            continue;
        }
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        last_space = false;
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{missing, normalize, REQUIRED};

    fn filled(mark: &str) -> String {
        let mut body = String::from("## Summary\n\nthing\n\n## Rules attestation\n\n");
        for phrase in REQUIRED {
            body.push_str("- [");
            body.push_str(mark);
            body.push_str("] ");
            body.push_str(phrase);
            body.push('\n');
        }
        body
    }

    #[test]
    fn accepts_a_fully_ticked_checklist() {
        assert!(missing(&filled("x")).is_empty());
    }

    #[test]
    fn accepts_backticked_and_rewrapped_phrases() {
        let body = "- [x] I read all of `.rules/` before opening this PR\n  and before marking it ready\n- [x] `AGENTS.md`, `README.md` and `.rules/` are accurate for this change, or N/A with a reason below\n- [x] Local pre-prod gates in `.rules/20-pre-prod-local.md` all passed\n- [x] Version bumped per `.rules/50-versioning.md`\n";
        assert!(missing(body).is_empty(), "{:?}", missing(body));
    }

    #[test]
    fn rejects_unchecked_boxes_with_a_pointed_message() {
        let failures = missing(&filled(" "));
        assert_eq!(failures.len(), REQUIRED.len());
        assert!(failures.iter().all(|f| f.contains("left unchecked")));
    }

    #[test]
    fn rejects_an_empty_body() {
        let failures = missing("   \n\n");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("empty"));
    }

    #[test]
    fn rejects_a_body_without_the_checklist() {
        let failures = missing("## Summary\n\nlgtm\n");
        assert_eq!(failures.len(), REQUIRED.len());
        assert!(failures.iter().all(|f| f.contains("missing from the body")));
    }

    #[test]
    fn normalize_collapses_whitespace_and_markdown() {
        assert_eq!(normalize("A  `b`\n *c*"), "a b c");
    }
}
