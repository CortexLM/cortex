//! Single-source-of-truth workspace version.
//!
//! The only place a version is written by hand is `[workspace.package] version`
//! in the root `Cargo.toml`. Every member inherits it with
//! `version.workspace = true`, and `Cargo.lock` is rewritten from it.
//!
//! Level is derived from Conventional Commit subjects on the branch:
//! breaking → major, `feat` → minor, anything else → patch. While the major
//! component is `0`, a breaking change maps to **minor** (cargo treats `0.y`
//! as the compatibility unit), so the repo does not jump to `1.0.0` by
//! accident. Rules text: `.rules/50-versioning.md`.

use clap::Subcommand;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Paths whose changes alone do not require a version bump.
///
/// `images.yml` commits digest pins straight to `main`; a PR that only moves
/// pins or the metadata lockfile is a machine artifact, not a release.
const BUMP_EXEMPT_PREFIXES: &[&str] = &["deploy/pins/", "deploy/digests/", "metadata/"];

/// Conventional Commit types that mean "new behaviour" (minor).
const FEATURE_TYPES: &[&str] = &["feat"];

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print the workspace version.
    Show,
    /// Verify members inherit the version and `Cargo.lock` agrees.
    Check,
    /// Rewrite `Cargo.toml` + `Cargo.lock` to the next version.
    Bump {
        /// `auto` (from Conventional Commits), `patch`, `minor`, or `major`.
        #[arg(long, default_value = "auto")]
        level: String,
        /// Base ref used by `--level auto` to collect commit subjects.
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    /// CI gate: fail unless this branch bumped the version versus `--base`.
    VerifyBump {
        /// Base ref (usually `origin/main`).
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Patch,
    Minor,
    Major,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        };
        f.write_str(s)
    }
}

impl Level {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "patch" => Ok(Self::Patch),
            "minor" => Ok(Self::Minor),
            "major" => Ok(Self::Major),
            other => Err(format!(
                "unknown level {other:?} (expected auto, patch, minor, or major)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl SemVer {
    fn parse(raw: &str) -> Result<Self, String> {
        let mut parts = raw.trim().split('.');
        let mut next = |what: &str| -> Result<u64, String> {
            parts
                .next()
                .ok_or_else(|| format!("version {raw:?} is missing its {what} component"))?
                .parse::<u64>()
                .map_err(|e| format!("version {raw:?} has a non-numeric {what}: {e}"))
        };
        let major = next("major")?;
        let minor = next("minor")?;
        let patch = next("patch")?;
        if parts.next().is_some() {
            return Err(format!(
                "version {raw:?} has more than three components; pre-release and build \
                 metadata are not supported by this scheme"
            ));
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    fn bumped(self, level: Level) -> Self {
        match level {
            Level::Major => Self {
                major: self.major.saturating_add(1),
                minor: 0,
                patch: 0,
            },
            Level::Minor => Self {
                major: self.major,
                minor: self.minor.saturating_add(1),
                patch: 0,
            },
            Level::Patch => Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch.saturating_add(1),
            },
        }
    }

    /// Which level separates `self` (older) from `newer`.
    fn level_to(self, newer: Self) -> Option<Level> {
        if newer.major != self.major {
            Some(Level::Major)
        } else if newer.minor != self.minor {
            Some(Level::Minor)
        } else if newer.patch != self.patch {
            Some(Level::Patch)
        } else {
            None
        }
    }
}

/// Run the `version` subcommand.
///
/// # Errors
///
/// Returns an error when the manifest cannot be parsed, a member does not
/// inherit the workspace version, or a requested gate fails.
pub fn run(root: &Path, action: Option<&Action>) -> Result<(), String> {
    match action {
        None | Some(Action::Show) => {
            println!("{}", read_version(root)?);
            Ok(())
        }
        Some(Action::Check) => check(root),
        Some(Action::Bump { level, base }) => bump(root, level, base),
        Some(Action::VerifyBump { base }) => verify_bump(root, base),
    }
}

fn manifest_path(root: &Path) -> std::path::PathBuf {
    root.join("Cargo.toml")
}

/// Read `[workspace.package] version` from the root manifest.
fn read_version(root: &Path) -> Result<SemVer, String> {
    let path = manifest_path(root);
    let body = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let raw = extract_workspace_version(&body)
        .ok_or_else(|| format!("no [workspace.package] version in {}", path.display()))?;
    SemVer::parse(&raw)
}

fn extract_workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("version") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_owned());
        }
    }
    None
}

/// Every workspace member package name (they all inherit the version).
fn member_names(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for manifest in member_manifests(root)? {
        let body = fs::read_to_string(&manifest)
            .map_err(|e| format!("read {}: {e}", manifest.display()))?;
        let name = package_name(&body)
            .ok_or_else(|| format!("no [package] name in {}", manifest.display()))?;
        names.insert(name);
    }
    Ok(names)
}

fn member_manifests(root: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = vec![root.join("xtask/Cargo.toml")];
    for area in ["crates", "bins"] {
        let dir = root.join(area);
        if !dir.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("dir entry under {area}: {e}"))?;
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                out.push(manifest);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_owned());
            }
        }
    }
    None
}

/// Intra-workspace `path = "…"` deps that also pin `version = "…"`.
///
/// Such a pin makes `version bump` unresolvable: the sibling crate moves to the
/// new version while the requirement still asks for the old one.
fn pinned_path_deps(manifest: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.contains("path = \"") {
            continue;
        }
        if !trimmed.contains("version = \"") {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=') {
            out.push(name.trim().to_owned());
        }
    }
    out
}

fn check(root: &Path) -> Result<(), String> {
    let version = read_version(root)?;
    let mut failures = Vec::new();

    for manifest in member_manifests(root)? {
        let body = fs::read_to_string(&manifest)
            .map_err(|e| format!("read {}: {e}", manifest.display()))?;
        let inherits = body
            .lines()
            .map(str::trim)
            .any(|l| l == "version.workspace = true" || l == "version = { workspace = true }");
        if !inherits {
            let rel = manifest.strip_prefix(root).unwrap_or(&manifest);
            failures.push(format!(
                "{}: must inherit the workspace version (`version.workspace = true`)",
                rel.display()
            ));
        }
        for dep in pinned_path_deps(&body) {
            let rel = manifest.strip_prefix(root).unwrap_or(&manifest);
            failures.push(format!(
                "{}: path dependency `{dep}` pins a literal version; drop it so the \
                 workspace version can move (members are `publish = false`)",
                rel.display()
            ));
        }
    }

    let lock_path = root.join("Cargo.lock");
    let lock =
        fs::read_to_string(&lock_path).map_err(|e| format!("read {}: {e}", lock_path.display()))?;
    let members = member_names(root)?;
    let want = version.to_string();
    for (name, locked) in locked_member_versions(&lock, &members) {
        if locked != want {
            failures.push(format!(
                "Cargo.lock: {name} is {locked}, workspace version is {want} \
                 (run `cargo run -p xtask -- version bump --level patch`, or `cargo update -w`)"
            ));
        }
    }

    if failures.is_empty() {
        println!(
            "version check: OK ({version}, {} members inherit, Cargo.lock in sync)",
            members.len()
        );
        Ok(())
    } else {
        Err(format!(
            "version check failed ({}):\n{}",
            failures.len(),
            failures
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

/// Locked versions of path members (`[[package]]` blocks without a `source`).
fn locked_member_versions(lock: &str, members: &BTreeSet<String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let mut name = None;
        let mut locked = None;
        let mut vendored = false;
        for line in block.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("name = ") {
                name = Some(rest.trim_matches('"').to_owned());
            } else if let Some(rest) = trimmed.strip_prefix("version = ") {
                locked = Some(rest.trim_matches('"').to_owned());
            } else if trimmed.starts_with("source = ") || trimmed.starts_with("checksum = ") {
                vendored = true;
            }
        }
        if vendored {
            continue;
        }
        if let (Some(name), Some(locked)) = (name, locked) {
            if members.contains(&name) {
                out.push((name, locked));
            }
        }
    }
    out
}

fn bump(root: &Path, level: &str, base: &str) -> Result<(), String> {
    let current = read_version(root)?;
    let level = if level == "auto" {
        let subjects = commit_subjects(root, base)?;
        if subjects.is_empty() {
            return Err(format!(
                "no commits between {base} and HEAD; pass an explicit --level"
            ));
        }
        required_level(&subjects, current)
    } else {
        Level::parse(level)?
    };
    let next = current.bumped(level);
    write_version(root, current, next)?;
    println!("version bump: {current} -> {next} ({level})");
    Ok(())
}

fn write_version(root: &Path, current: SemVer, next: SemVer) -> Result<(), String> {
    let path = manifest_path(root);
    let body = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let from = format!("version = \"{current}\"");
    let to = format!("version = \"{next}\"");
    let mut out = String::with_capacity(body.len());
    let mut in_section = false;
    let mut replaced = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
        }
        if in_section && !replaced && trimmed == from {
            out.push_str(&to);
            replaced = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !replaced {
        return Err(format!(
            "could not find `{from}` under [workspace.package] in {}",
            path.display()
        ));
    }
    fs::write(&path, out).map_err(|e| format!("write {}: {e}", path.display()))?;

    let lock_path = root.join("Cargo.lock");
    let lock =
        fs::read_to_string(&lock_path).map_err(|e| format!("read {}: {e}", lock_path.display()))?;
    let members = member_names(root)?;
    let (rewritten, count) = rewrite_lock(&lock, &members, current, next);
    fs::write(&lock_path, rewritten).map_err(|e| format!("write {}: {e}", lock_path.display()))?;
    println!("Cargo.lock: {count} member entries rewritten to {next}");
    Ok(())
}

/// Rewrite path-member versions in `Cargo.lock` without invoking cargo.
fn rewrite_lock(
    lock: &str,
    members: &BTreeSet<String>,
    current: SemVer,
    next: SemVer,
) -> (String, usize) {
    let from = format!("version = \"{current}\"");
    let to = format!("version = \"{next}\"");
    let mut out = String::with_capacity(lock.len());
    let mut name: Option<String> = None;
    let mut vendored = false;
    let mut count = 0usize;
    for line in lock.lines() {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            name = None;
            vendored = false;
        } else if let Some(rest) = trimmed.strip_prefix("name = ") {
            name = Some(rest.trim_matches('"').to_owned());
        } else if trimmed.starts_with("source = ") || trimmed.starts_with("checksum = ") {
            vendored = true;
        }
        let is_member = name.as_ref().is_some_and(|n| members.contains(n));
        if trimmed == from && is_member && !vendored {
            out.push_str(&to);
            count = count.saturating_add(1);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, count)
}

fn verify_bump(root: &Path, base: &str) -> Result<(), String> {
    let merge_base = git(root, &["merge-base", base, "HEAD"]).map_err(|e| {
        format!("{e}\nhint: CI needs full history (actions/checkout with fetch-depth: 0)")
    })?;
    let base_manifest = git(root, &["show", &format!("{merge_base}:Cargo.toml")])?;
    let base_version = extract_workspace_version(&base_manifest)
        .ok_or_else(|| format!("no [workspace.package] version at {merge_base}"))?;
    let base_version = SemVer::parse(&base_version)?;
    let current = read_version(root)?;

    let changed = changed_files(root, &merge_base)?;
    if changed.is_empty() {
        println!("version verify-bump: no changed files vs {base}; nothing to gate");
        return Ok(());
    }
    if changed.iter().all(|f| is_bump_exempt(f)) {
        println!(
            "version verify-bump: {} changed file(s), all machine-pin paths; bump not required",
            changed.len()
        );
        return Ok(());
    }

    let Some(actual) = base_version.level_to(current) else {
        return Err(format!(
            "version was not bumped: still {current} on both {base} and this branch.\n\
             Run `cargo run -p xtask -- version bump` (or `--level patch|minor|major`) and \
             commit Cargo.toml + Cargo.lock. See .rules/50-versioning.md."
        ));
    };
    if current < base_version {
        return Err(format!(
            "version went backwards: {base} is {base_version}, this branch is {current}"
        ));
    }

    let subjects = commit_subjects(root, &merge_base)?;
    let required = required_level(&subjects, base_version);
    if actual < required {
        return Err(format!(
            "version bump too small: {base_version} -> {current} is a {actual} bump, but the \
             Conventional Commit subjects on this branch require at least a {required} bump.\n\
             Run `cargo run -p xtask -- version bump --level {required}`. See .rules/50-versioning.md."
        ));
    }

    println!(
        "version verify-bump: OK {base_version} -> {current} ({actual} bump, {required} required)"
    );
    Ok(())
}

fn is_bump_exempt(path: &str) -> bool {
    BUMP_EXEMPT_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn changed_files(root: &Path, merge_base: &str) -> Result<Vec<String>, String> {
    let out = git(root, &["diff", "--name-only", merge_base, "HEAD"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn commit_subjects(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let out = git(
        root,
        &[
            "log",
            "--no-merges",
            "--format=%s%n%b%n--%%--",
            &format!("{base}..HEAD"),
        ],
    )?;
    Ok(out
        .split("--%--")
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Smallest bump the branch's Conventional Commits justify.
///
/// While `current.major == 0`, breaking changes map to minor: `0.y` is the
/// cargo compatibility unit, so a `0.x` repo must not auto-promote to `1.0.0`.
fn required_level(commits: &[String], current: SemVer) -> Level {
    let mut level = Level::Patch;
    for commit in commits {
        let mut lines = commit.lines();
        let subject = lines.next().unwrap_or_default().trim();
        let breaking_body = commit.contains("BREAKING CHANGE");
        let Some((head, _)) = subject.split_once(':') else {
            if breaking_body {
                level = level.max(breaking_level(current));
            }
            continue;
        };
        let breaking = head.ends_with('!') || breaking_body;
        let ty = head
            .trim_end_matches('!')
            .split_once('(')
            .map_or(head.trim_end_matches('!'), |(t, _)| t)
            .trim();
        if breaking {
            level = level.max(breaking_level(current));
        } else if FEATURE_TYPES.contains(&ty) {
            level = level.max(Level::Minor);
        }
    }
    level
}

fn breaking_level(current: SemVer) -> Level {
    if current.major == 0 {
        Level::Minor
    } else {
        Level::Major
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("run git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO_ONE: SemVer = SemVer {
        major: 0,
        minor: 1,
        patch: 0,
    };

    #[test]
    fn parses_and_renders_semver() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.to_string(), "1.2.3");
        assert!(SemVer::parse("1.2").is_err());
        assert!(SemVer::parse("1.2.3-rc.1").is_err());
    }

    #[test]
    fn bump_levels_reset_lower_components() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.bumped(Level::Patch).to_string(), "1.2.4");
        assert_eq!(v.bumped(Level::Minor).to_string(), "1.3.0");
        assert_eq!(v.bumped(Level::Major).to_string(), "2.0.0");
    }

    #[test]
    fn level_to_detects_the_bump() {
        let a = SemVer::parse("0.1.0").unwrap();
        assert_eq!(a.level_to(a), None);
        assert_eq!(
            a.level_to(SemVer::parse("0.1.1").unwrap()),
            Some(Level::Patch)
        );
        assert_eq!(
            a.level_to(SemVer::parse("0.2.0").unwrap()),
            Some(Level::Minor)
        );
        assert_eq!(
            a.level_to(SemVer::parse("1.0.0").unwrap()),
            Some(Level::Major)
        );
    }

    #[test]
    fn feat_requires_minor_and_fix_requires_patch() {
        let feat = vec![String::from("feat(gateway): add seal route")];
        assert_eq!(required_level(&feat, ZERO_ONE), Level::Minor);
        let fix = vec![String::from("fix(validator): stop double submit")];
        assert_eq!(required_level(&fix, ZERO_ONE), Level::Patch);
        let chore = vec![String::from("chore: tidy")];
        assert_eq!(required_level(&chore, ZERO_ONE), Level::Patch);
    }

    #[test]
    fn breaking_stays_minor_while_major_is_zero() {
        let bang = vec![String::from("feat(bundle)!: change leaf bytes")];
        assert_eq!(required_level(&bang, ZERO_ONE), Level::Minor);
        let one_two = SemVer::parse("1.2.0").unwrap();
        assert_eq!(required_level(&bang, one_two), Level::Major);

        let footer = vec![String::from(
            "refactor(chain): rework client\n\nBREAKING CHANGE: drops the old trait",
        )];
        assert_eq!(required_level(&footer, one_two), Level::Major);
    }

    #[test]
    fn extracts_version_from_workspace_package_only() {
        let manifest = "[workspace]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"0.4.2\"\nedition = \"2021\"\n";
        assert_eq!(
            extract_workspace_version(manifest).as_deref(),
            Some("0.4.2")
        );
        let no_section = "[package]\nversion = \"9.9.9\"\n";
        assert_eq!(extract_workspace_version(no_section), None);
    }

    #[test]
    fn rewrites_only_path_members() {
        let lock = "[[package]]\nname = \"aggregate\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"0.1.0\"\nsource = \"registry+https://x\"\nchecksum = \"abc\"\n";
        let members = BTreeSet::from([String::from("aggregate")]);
        let (out, count) = rewrite_lock(lock, &members, ZERO_ONE, SemVer::parse("0.2.0").unwrap());
        assert_eq!(count, 1);
        assert!(out.contains("name = \"aggregate\"\nversion = \"0.2.0\""));
        assert!(out.contains("name = \"serde\"\nversion = \"0.1.0\""));
    }

    #[test]
    fn locked_member_versions_skips_registry_crates() {
        let lock = "[[package]]\nname = \"bundle\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"bundle-ext\"\nversion = \"3.0.0\"\nsource = \"registry+x\"\n";
        let members = BTreeSet::from([String::from("bundle"), String::from("bundle-ext")]);
        let found = locked_member_versions(lock, &members);
        assert_eq!(found, vec![(String::from("bundle"), String::from("0.1.0"))]);
    }

    #[test]
    fn flags_path_deps_that_pin_a_version() {
        let manifest = "[dependencies]\nbundle = { path = \"../bundle\" }\nprism-budget = { version = \"0.1.0\", path = \"../prism-budget\" }\nserde = { version = \"1\" }\n# old = { version = \"0.1.0\", path = \"../old\" }\n";
        assert_eq!(
            pinned_path_deps(manifest),
            vec![String::from("prism-budget")]
        );
    }

    #[test]
    fn pin_only_paths_are_exempt() {
        assert!(is_bump_exempt("deploy/pins/staging.json"));
        assert!(is_bump_exempt("metadata/testnet.lock"));
        assert!(!is_bump_exempt("crates/bundle/src/types.rs"));
    }

    #[test]
    fn workspace_manifest_is_consistent() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = root
            .parent()
            .map_or_else(std::path::PathBuf::new, Path::to_path_buf);
        super::check(&root).expect("workspace version must be consistent");
    }
}
