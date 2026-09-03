//! Repo-discipline gate for the `.rules/` agent contract.
//!
//! Enforces, in one place, the things a coding agent is otherwise free to let
//! rot:
//!
//! 1. `.rules/` is complete and the relocated frozen contracts are present.
//! 2. There is no `docs/` tree (README + `.rules/` are the only doc surfaces).
//! 3. `AGENTS.md` and `README.md` point at `.rules/` up front.
//! 4. The PR template carries the exact attestations `pr-check` requires.
//! 5. `.rules/20-pre-prod-local.md` lists every command CI actually runs.
//! 6. No markdown file links to a path that does not exist.
//! 7. Numbered rules stay linked from the overview / entry points, and GitHub
//!    workflows never transport a BIP39 mnemonic (`PROD_ROTATE_MNEMONIC` and
//!    any `secrets.*MNEMONIC*` name are banned).

use crate::pr_check::REQUIRED;
use crate::CONTRACTS_DIR;
use std::fs;
use std::path::{Path, PathBuf};

/// Numbered rules files. Every one is mandatory reading before a PR.
const RULES_FILES: &[&str] = &[
    "00-overview.md",
    "10-maintenance.md",
    "20-pre-prod-local.md",
    "30-pr.md",
    "40-agents.md",
    "50-versioning.md",
    "60-naming.md",
    "70-secrets-mnemonics.md",
];

/// Entry points that must keep naming the mnemonic / wallet-JSON rule.
const REQUIRED_RULE_POINTERS: &[(&str, &str)] = &[
    ("AGENTS.md", "70-secrets-mnemonics.md"),
    (".rules/40-agents.md", "70-secrets-mnemonics.md"),
    (
        ".rules/contracts/THREAT_MODEL.md",
        "70-secrets-mnemonics.md",
    ),
];

/// Banned GitHub Actions secret name used as a mnemonic transport.
const BANNED_MNEMONIC_SECRET: &str = "PROD_ROTATE_MNEMONIC";

/// Pin kept in both ignore files so mnemonic paths never enter git or images.
const MNEMONIC_IGNORE_PIN: &str = "**/*mnemonic*";

/// Frozen contracts relocated out of the deleted `docs/` tree.
const CONTRACT_FILES: &[&str] = &[
    "README.md",
    "BUNDLE_SPEC.md",
    "BUNDLE_SPEC_CHECKLIST.md",
    "DESIGN_CHALLENGE.md",
    "DESIGN_CHALLENGE_CHECKLIST.md",
    "PRISM.md",
    "PRISM_RECIPE.md",
    "THREAT_MODEL.md",
    "external-miner/README.md",
];

/// Markdown surfaces whose relative links must resolve.
const LINKED_MARKDOWN: &[&str] = &[
    "README.md",
    "AGENTS.md",
    "SECURITY.md",
    "deploy/README.md",
    "deploy/AGENTS.md",
];

/// The agent entry point that makes `.rules/` unmissable in Cursor.
const CURSOR_RULE: &str = ".cursor/rules/00-read-dot-rules.mdc";

/// Where the local pre-prod gate list lives.
const PRE_PROD: &str = ".rules/20-pre-prod-local.md";

const PR_TEMPLATE: &str = ".github/PULL_REQUEST_TEMPLATE.md";

const CI_WORKFLOW: &str = ".github/workflows/ci.yml";

/// Run the `.rules/` discipline gate.
///
/// # Errors
///
/// Returns a multi-line error listing every drift found.
pub fn run(root: &Path) -> Result<(), String> {
    let mut failures = Vec::new();

    if root.join("docs").exists() {
        failures.push(String::from(
            "docs/ exists again: README.md is the only human doc surface and .rules/ is the \
             agent contract (see .rules/10-maintenance.md)",
        ));
    }

    for name in RULES_FILES {
        require_nonempty(root, &format!(".rules/{name}"), &mut failures);
    }
    for name in CONTRACT_FILES {
        require_nonempty(root, &format!("{CONTRACTS_DIR}/{name}"), &mut failures);
    }
    require_nonempty(root, CURSOR_RULE, &mut failures);
    require_nonempty(root, PR_TEMPLATE, &mut failures);

    check_pointer(root, "AGENTS.md", 25, &mut failures);
    check_pointer(root, "README.md", usize::MAX, &mut failures);
    check_cursor_rule(root, &mut failures);
    check_overview_lists_every_rule(root, &mut failures);
    check_required_rule_pointers(root, &mut failures);
    check_mnemonic_ignore_pins(root, &mut failures);
    check_banned_mnemonic_transport(root, &mut failures)?;
    check_pr_template(root, &mut failures);
    check_ci_commands(root, &mut failures)?;
    check_links(root, &mut failures)?;

    if failures.is_empty() {
        println!(
            "rules-check: OK ({} rules files, {} contracts, {} attestations, links resolve)",
            RULES_FILES.len(),
            CONTRACT_FILES.len(),
            REQUIRED.len()
        );
        Ok(())
    } else {
        Err(format!(
            "rules-check failed ({}):\n{}",
            failures.len(),
            failures
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

fn require_nonempty(root: &Path, rel: &str, failures: &mut Vec<String>) {
    let path = root.join(rel);
    match fs::metadata(&path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => {}
        Ok(_) => failures.push(format!("{rel} is empty or not a file")),
        Err(e) => failures.push(format!("{rel} is missing ({e})")),
    }
}

/// `AGENTS.md` must send agents to `.rules/` before anything else.
fn check_pointer(root: &Path, rel: &str, within_lines: usize, failures: &mut Vec<String>) {
    let path = root.join(rel);
    let Ok(body) = fs::read_to_string(&path) else {
        return; // already reported by require_nonempty
    };
    let head: String = body
        .lines()
        .take(within_lines.min(body.lines().count()))
        .collect::<Vec<_>>()
        .join("\n");
    if !head.contains(".rules/") {
        failures.push(format!(
            "{rel} must point at `.rules/` within its first {within_lines} lines"
        ));
    }
}

fn check_overview_lists_every_rule(root: &Path, failures: &mut Vec<String>) {
    let rel = ".rules/00-overview.md";
    let Ok(body) = fs::read_to_string(root.join(rel)) else {
        return;
    };
    for name in RULES_FILES {
        if !body.contains(name) {
            failures.push(format!("{rel} must name numbered rule `{name}`"));
        }
    }
}

fn check_required_rule_pointers(root: &Path, failures: &mut Vec<String>) {
    for (rel, needle) in REQUIRED_RULE_POINTERS {
        let Ok(body) = fs::read_to_string(root.join(rel)) else {
            failures.push(format!("{rel} is missing (must link `{needle}`)"));
            continue;
        };
        if !body.contains(needle) {
            failures.push(format!("{rel} must link numbered rule `{needle}`"));
        }
    }
}

fn check_mnemonic_ignore_pins(root: &Path, failures: &mut Vec<String>) {
    for rel in [".dockerignore", ".gitignore"] {
        let Ok(body) = fs::read_to_string(root.join(rel)) else {
            failures.push(format!(
                "{rel} is missing (must keep `{MNEMONIC_IGNORE_PIN}`)"
            ));
            continue;
        };
        if !body.contains(MNEMONIC_IGNORE_PIN) {
            failures.push(format!("{rel} must keep `{MNEMONIC_IGNORE_PIN}`"));
        }
    }
    let Ok(gitignore) = fs::read_to_string(root.join(".gitignore")) else {
        return;
    };
    if !gitignore.contains("deploy/secrets/*") {
        failures.push(String::from(
            ".gitignore must keep `deploy/secrets/*` untracked (except documented README placeholders)",
        ));
    }
    if !gitignore.contains("!.rules/70-secrets-mnemonics.md") {
        failures.push(String::from(
            ".gitignore must un-ignore `.rules/70-secrets-mnemonics.md` (documentation, not a secret file)",
        ));
    }
}

/// Scan GitHub workflow files for banned mnemonic transports.
///
/// # Errors
///
/// Returns an I/O error when `.github/workflows` cannot be read.
fn check_banned_mnemonic_transport(root: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let dir = root.join(".github/workflows");
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        let is_yaml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "yml" || e == "yaml");
        if !is_yaml {
            continue;
        }
        let body =
            fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if body.contains(BANNED_MNEMONIC_SECRET) {
            failures.push(format!(
                "{} uses banned mnemonic transport `{BANNED_MNEMONIC_SECRET}`",
                rel.display()
            ));
        }
        if let Some(name) = github_mnemonic_secret_name(&body) {
            if name != BANNED_MNEMONIC_SECRET {
                failures.push(format!(
                    "{} uses GitHub secret `{name}` as a mnemonic transport \
                     (secrets.*MNEMONIC* is banned)",
                    rel.display()
                ));
            }
        }
    }
    Ok(())
}

/// First `secrets.NAME` in `body` whose name contains `MNEMONIC`.
fn github_mnemonic_secret_name(body: &str) -> Option<String> {
    const PREFIX: &str = "secrets.";
    let mut rest = body;
    while let Some(idx) = rest.find(PREFIX) {
        let after = rest.get(idx.saturating_add(PREFIX.len())..)?;
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.to_ascii_uppercase().contains("MNEMONIC") {
            return Some(name);
        }
        rest = after;
    }
    None
}

fn check_cursor_rule(root: &Path, failures: &mut Vec<String>) {
    let path = root.join(CURSOR_RULE);
    let Ok(body) = fs::read_to_string(&path) else {
        return;
    };
    if !body.contains("alwaysApply: true") {
        failures.push(format!(
            "{CURSOR_RULE} must set `alwaysApply: true` so agents always see the rules gate"
        ));
    }
    if !body.contains(".rules/") {
        failures.push(format!("{CURSOR_RULE} must require reading `.rules/`"));
    }
    for name in RULES_FILES {
        if !body.contains(name) {
            failures.push(format!("{CURSOR_RULE} must list numbered rule `{name}`"));
        }
    }
}

fn check_pr_template(root: &Path, failures: &mut Vec<String>) {
    let path = root.join(PR_TEMPLATE);
    let Ok(body) = fs::read_to_string(&path) else {
        return;
    };
    let squashed = squash(&body);
    for phrase in REQUIRED {
        if !squashed.contains(&squash(phrase)) {
            failures.push(format!(
                "{PR_TEMPLATE} is missing the attestation `pr-check` requires: {phrase}"
            ));
        }
    }
}

/// Every `cargo` / `bash` command CI runs must be listed for local use.
fn check_ci_commands(root: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let ci_path = root.join(CI_WORKFLOW);
    let ci = fs::read_to_string(&ci_path).map_err(|e| format!("read {CI_WORKFLOW}: {e}"))?;
    let doc_path = root.join(PRE_PROD);
    let Ok(doc) = fs::read_to_string(&doc_path) else {
        return Ok(());
    };
    let doc = squash(&doc);
    for cmd in ci_commands(&ci) {
        if !doc.contains(&squash(&cmd)) {
            failures.push(format!(
                "{PRE_PROD} does not list a command CI runs: `{cmd}`"
            ));
        }
    }
    Ok(())
}

/// Single-line `run:` steps that invoke cargo or a repo script.
fn ci_commands(workflow: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in workflow.lines() {
        let Some(rest) = line.trim().strip_prefix("run: ") else {
            continue;
        };
        let cmd = rest.trim();
        if cmd.starts_with("cargo ") || cmd.starts_with("bash deploy/") {
            out.push(cmd.to_owned());
        }
    }
    out
}

fn check_links(root: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let mut files: Vec<PathBuf> = LINKED_MARKDOWN
        .iter()
        .map(|rel| root.join(rel))
        .filter(|p| p.is_file())
        .collect();
    collect_markdown(&root.join(".rules"), &mut files)?;
    files.sort();

    for file in files {
        let body =
            fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
        let dir = file.parent().unwrap_or(root);
        let rel_file = file.strip_prefix(root).unwrap_or(&file);
        for target in link_targets(&body) {
            let resolved = dir.join(&target);
            if !resolved.exists() {
                failures.push(format!(
                    "{}: link target does not exist: {target}",
                    rel_file.display()
                ));
            }
        }
    }
    Ok(())
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Relative markdown link targets, with anchors and external schemes dropped.
fn link_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = body.chars().collect();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] != ']' || bytes[i + 1] != '(' {
            i = i.saturating_add(1);
            continue;
        }
        let mut j = i.saturating_add(2);
        let mut target = String::new();
        while j < bytes.len() && bytes[j] != ')' {
            target.push(bytes[j]);
            j = j.saturating_add(1);
        }
        i = j.saturating_add(1);
        let target = target.split_whitespace().next().unwrap_or("").trim();
        let target = target.split('#').next().unwrap_or("").trim();
        if target.is_empty() {
            continue;
        }
        let external = target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
            || target.starts_with('<');
        if external {
            continue;
        }
        out.push(target.to_owned());
    }
    out
}

/// Collapse whitespace and drop markdown emphasis for tolerant comparisons.
fn squash(text: &str) -> String {
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
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }

    #[test]
    fn link_targets_skips_anchors_and_urls() {
        let body = "[a](./x.md) [b](https://example.com) [c](#frag) [d](../y.md#z) [e]()";
        assert_eq!(link_targets(body), vec!["./x.md", "../y.md"]);
    }

    #[test]
    fn ci_commands_picks_cargo_and_scripts() {
        let wf = "      - name: t\n        run: cargo fmt --all -- --check\n      - name: u\n        run: bash deploy/scripts/assert-compose-matrix.sh\n      - name: v\n        run: echo skip\n";
        assert_eq!(
            ci_commands(wf),
            vec![
                String::from("cargo fmt --all -- --check"),
                String::from("bash deploy/scripts/assert-compose-matrix.sh"),
            ]
        );
    }

    #[test]
    fn every_rules_file_is_numbered_and_unique() {
        let mut sorted = RULES_FILES.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, RULES_FILES.to_vec(), "keep RULES_FILES sorted");
        assert!(RULES_FILES
            .iter()
            .all(|f| Path::new(f).extension().is_some_and(|e| e == "md")));
    }

    #[test]
    fn secrets_rule_is_required_reading_and_linked() {
        assert!(
            RULES_FILES.contains(&"70-secrets-mnemonics.md"),
            "add 70-secrets-mnemonics.md to RULES_FILES"
        );
        let pointers: Vec<(&str, &str)> = REQUIRED_RULE_POINTERS.to_vec();
        assert!(
            pointers.contains(&("AGENTS.md", "70-secrets-mnemonics.md")),
            "AGENTS.md must keep a pointer at the secrets rule"
        );
        assert!(pointers.contains(&(".rules/40-agents.md", "70-secrets-mnemonics.md")));
        assert!(pointers.contains(&(
            ".rules/contracts/THREAT_MODEL.md",
            "70-secrets-mnemonics.md"
        )));
        assert_eq!(BANNED_MNEMONIC_SECRET, "PROD_ROTATE_MNEMONIC");
        assert_eq!(MNEMONIC_IGNORE_PIN, "**/*mnemonic*");
    }

    #[test]
    fn github_mnemonic_secret_name_flags_actions_secrets_only() {
        assert_eq!(
            github_mnemonic_secret_name("password: ${{ secrets.PROD_ROTATE_MNEMONIC }}"),
            Some(String::from("PROD_ROTATE_MNEMONIC"))
        );
        assert_eq!(
            github_mnemonic_secret_name("env: MNEMONIC_FILE=/run/base/x"),
            None
        );
        assert_eq!(
            github_mnemonic_secret_name("password: ${{ secrets.GITHUB_TOKEN }}"),
            None
        );
    }

    #[test]
    fn this_repo_passes_the_gate() {
        super::run(&workspace_root()).expect("rules-check must pass on this workspace");
    }
}
