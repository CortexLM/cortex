//! Topic-scoped artefact store: `{root}/{topic_id}/{submission_id}.zip`.
//!
//! ```text
//! manifest.json      what this bundle is (ids, digests, primary, promoted)
//! checklist.json     the rule items with evidence (rule version bound)
//! report.json        runner-authored measurement (absent on a pre-spend reject)
//! baseline_ref.json  the sealed baseline this run was compared against
//! artifact/…         the miner's tree as inspected
//! logs/…             runner logs (harness stdout, guest console, judge transcript)
//! ```
//!
//! Beside the zips: `best.json` (current best pointer) and `events.jsonl`
//! (append-only public promotion events). Every path component is validated
//! before it touches the filesystem, the zip is written deterministically
//! (sorted entries, fixed timestamps), and nothing here knows a challenge.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use proof_canon::is_slug;
use proof_rlm::{ArtifactFile, Checklist, CustomRunReport, LogFile};
use proof_task::{ProofPin, TopicDocument};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Env var naming the artefact root directory.
pub const ARTEFACT_ROOT_ENV: &str = "PROOF_ARTEFACT_ROOT";

/// Default artefact root.
pub const DEFAULT_ARTEFACT_ROOT: &str = "/artefacts";

/// Only accepted `manifest.json` schema.
pub const ARTEFACT_MANIFEST_SCHEMA: u32 = 1;

/// Manifest entry.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Checklist entry.
pub const CHECKLIST_FILE: &str = "checklist.json";
/// Report entry.
pub const REPORT_FILE: &str = "report.json";
/// Baseline reference entry.
pub const BASELINE_REF_FILE: &str = "baseline_ref.json";
/// Miner tree prefix.
pub const ARTIFACT_DIR: &str = "artifact";
/// Logs prefix.
pub const LOGS_DIR: &str = "logs";
/// Per-topic best pointer (next to the zips).
pub const BEST_FILE: &str = "best.json";
/// Per-topic public event log (append-only JSON lines).
pub const EVENTS_FILE: &str = "events.jsonl";

/// Longest entry path inside the zip.
pub const MAX_ENTRY_PATH: usize = 256;

/// The sealed baseline this run was compared against (public fields only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineRef {
    /// Topic id.
    pub topic_id: String,
    /// SHA-256 of the sealed baseline script.
    pub script_sha256: String,
    /// Commitment over the sealed metric vector.
    pub metrics_commitment: String,
    /// Holdout commitment of the topic.
    pub holdout_commitment: String,
    /// Eval image digest the host pins.
    pub eval_image_digest: String,
}

impl BaselineRef {
    /// Public seal references from the topic and pin.
    #[must_use]
    pub fn from_topic(topic: &TopicDocument, pin: &ProofPin) -> Self {
        Self {
            topic_id: topic.id.clone(),
            script_sha256: topic.baseline.script_sha256.clone(),
            metrics_commitment: topic.baseline.metrics_commitment.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
        }
    }
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtefactManifest {
    /// Must equal [`ARTEFACT_MANIFEST_SCHEMA`].
    pub schema_version: u32,
    /// Topic id.
    pub topic_id: String,
    /// Custom metric id.
    pub custom_id: String,
    /// Store row id (`pf_…`).
    pub submission_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Miner artefact digest.
    pub artifact_digest: String,
    /// Rule version the checklist ticked.
    pub rules_version: u32,
    /// Primary value, when a report exists.
    pub primary_value: Option<f64>,
    /// Whether the checklist was green (spend happened only if true).
    pub checklist_green: bool,
    /// Whether this run was promoted.
    pub promoted: bool,
    /// Every entry in the zip, sorted.
    pub entries: Vec<String>,
}

/// `best.json`: the topic's current best.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BestRef {
    /// Topic id.
    pub topic_id: String,
    /// Winning row id.
    pub submission_id: String,
    /// Winning frozen digest.
    pub submission_digest: String,
    /// Winning primary.
    pub primary_value: f64,
    /// Bar it cleared, when known.
    pub bar: Option<f64>,
    /// Zip file name next to this pointer.
    pub artefact: String,
}

/// One line of `events.jsonl`. Public data only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PublicEvent {
    /// A run was scored and its artefact written.
    Scored {
        /// Row id.
        submission_id: String,
        /// Primary value, when a report exists.
        primary_value: Option<f64>,
        /// Whether the checklist was green.
        checklist_green: bool,
    },
    /// A run became the topic's best.
    Promoted {
        /// Row id.
        submission_id: String,
        /// Primary value.
        primary_value: f64,
        /// Bar it cleared.
        bar: Option<f64>,
        /// Displaced best, if any.
        previous_best: Option<String>,
    },
}

/// Everything that goes into one zip, before the row id is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtefactBundle {
    /// Topic id.
    pub topic_id: String,
    /// Custom metric id.
    pub custom_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Miner artefact digest.
    pub artifact_digest: String,
    /// Whether the checklist was green under its rule version.
    pub checklist_green: bool,
    /// The checklist (red on a pre-spend reject).
    pub checklist: Checklist,
    /// Runner report (absent when the checklist refused spend).
    pub report: Option<CustomRunReport>,
    /// Sealed baseline reference.
    pub baseline_ref: BaselineRef,
    /// Miner tree.
    pub artifact: Vec<ArtifactFile>,
    /// Runner logs.
    pub logs: Vec<LogFile>,
}

/// Why a bundle could not be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArtefactError {
    /// Topic id is not a slug.
    #[error("artefact: topic id {0:?} is not a slug")]
    BadTopicId(String),
    /// Submission id is not `pf_` + 16 hex.
    #[error("artefact: submission id {0:?} is not a store row id")]
    BadSubmissionId(String),
    /// A tree / log path would escape or collide.
    #[error("artefact: entry path {0:?} is not a safe relative path")]
    BadEntryPath(String),
    /// Zip encoding failed.
    #[error("artefact: zip: {0}")]
    Zip(String),
    /// Filesystem failure.
    #[error("artefact: io: {0}")]
    Io(String),
}

fn parse_row_numeric(id: &str) -> Option<u64> {
    id.strip_prefix("pf_")
        .filter(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
}

fn is_row_id(id: &str) -> bool {
    parse_row_numeric(id).is_some()
}

/// Highest numeric `pf_` id among `{root}/{topic_id}/{submission_id}.zip`.
///
/// Missing root or no matching files → `None`. Non-zip names and malformed
/// ids are ignored so a leftover file cannot crash a restart.
#[must_use]
pub fn max_zip_numeric_id(root: &Path) -> Option<u64> {
    let topics = std::fs::read_dir(root).ok()?;
    let mut max = None;
    for topic in topics.flatten() {
        if !topic.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(topic.path()) else {
            continue;
        };
        for file in files.flatten() {
            if !file.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let name = file.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".zip")) else {
                continue;
            };
            if let Some(n) = parse_row_numeric(stem) {
                max = Some(max.map_or(n, |m: u64| m.max(n)));
            }
        }
    }
    max
}

/// Relative, no `.`/`..` segments, conservative charset, bounded length.
#[must_use]
pub fn is_safe_entry_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_ENTRY_PATH
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// `{root}/{topic_id}/{submission_id}.zip` after validating both ids.
///
/// # Errors
///
/// [`ArtefactError::BadTopicId`] / [`ArtefactError::BadSubmissionId`].
pub fn artefact_path(
    root: &Path,
    topic_id: &str,
    submission_id: &str,
) -> Result<PathBuf, ArtefactError> {
    if !is_slug(topic_id) {
        return Err(ArtefactError::BadTopicId(topic_id.to_owned()));
    }
    if !is_row_id(submission_id) {
        return Err(ArtefactError::BadSubmissionId(submission_id.to_owned()));
    }
    Ok(root.join(topic_id).join(format!("{submission_id}.zip")))
}

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644)
}

fn pretty<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_string_pretty(v)
        .unwrap_or_else(|_| "{}".into())
        .into_bytes()
}

impl ArtefactBundle {
    /// Sorted entry names this bundle will contain.
    #[must_use]
    pub fn entries(&self) -> Vec<String> {
        let mut out = vec![
            MANIFEST_FILE.to_owned(),
            CHECKLIST_FILE.to_owned(),
            BASELINE_REF_FILE.to_owned(),
        ];
        if self.report.is_some() {
            out.push(REPORT_FILE.to_owned());
        }
        out.extend(
            self.artifact
                .iter()
                .map(|f| format!("{ARTIFACT_DIR}/{}", f.path)),
        );
        out.extend(self.logs.iter().map(|l| format!("{LOGS_DIR}/{}", l.name)));
        out.sort();
        out
    }

    /// `manifest.json` for `submission_id`.
    #[must_use]
    pub fn manifest(&self, submission_id: &str, promoted: bool) -> ArtefactManifest {
        ArtefactManifest {
            schema_version: ARTEFACT_MANIFEST_SCHEMA,
            topic_id: self.topic_id.clone(),
            custom_id: self.custom_id.clone(),
            submission_id: submission_id.to_owned(),
            submission_digest: self.submission_digest.clone(),
            artifact_digest: self.artifact_digest.clone(),
            rules_version: self.checklist.rules_version,
            primary_value: self.report.as_ref().map(|r| r.primary_value),
            checklist_green: self.checklist_green,
            promoted,
            entries: self.entries(),
        }
    }

    /// Deterministic zip bytes.
    ///
    /// # Errors
    ///
    /// [`ArtefactError::BadEntryPath`] for an unsafe tree / log name,
    /// [`ArtefactError::Zip`] on encoder failure.
    pub fn zip_bytes(&self, submission_id: &str, promoted: bool) -> Result<Vec<u8>, ArtefactError> {
        let mut files: Vec<(String, Vec<u8>)> = vec![
            (
                MANIFEST_FILE.into(),
                pretty(&self.manifest(submission_id, promoted)),
            ),
            (CHECKLIST_FILE.into(), self.checklist.to_json().into_bytes()),
            (BASELINE_REF_FILE.into(), pretty(&self.baseline_ref)),
        ];
        if let Some(r) = &self.report {
            files.push((REPORT_FILE.into(), r.to_json().into_bytes()));
        }
        for f in &self.artifact {
            if !is_safe_entry_path(&f.path) {
                return Err(ArtefactError::BadEntryPath(f.path.clone()));
            }
            files.push((format!("{ARTIFACT_DIR}/{}", f.path), f.bytes.clone()));
        }
        for l in &self.logs {
            if !is_safe_entry_path(&l.name) || l.name.contains('/') {
                return Err(ArtefactError::BadEntryPath(l.name.clone()));
            }
            files.push((format!("{LOGS_DIR}/{}", l.name), l.bytes.clone()));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for pair in files.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(ArtefactError::BadEntryPath(pair[0].0.clone()));
            }
        }
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut cursor);
            for (name, bytes) in &files {
                w.start_file(name.as_str(), options())
                    .map_err(|e| ArtefactError::Zip(e.to_string()))?;
                w.write_all(bytes)
                    .map_err(|e| ArtefactError::Zip(e.to_string()))?;
            }
            w.finish().map_err(|e| ArtefactError::Zip(e.to_string()))?;
        }
        Ok(cursor.into_inner())
    }
}

/// A written zip: where it is and what it hashes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenArtefact {
    /// Zip path.
    pub path: PathBuf,
    /// SHA-256 hex of the zip bytes.
    pub sha256: String,
    /// Zip size.
    pub bytes: u64,
}

/// Filesystem store rooted at [`ARTEFACT_ROOT_ENV`] / [`DEFAULT_ARTEFACT_ROOT`].
#[derive(Debug, Clone)]
pub struct ArtefactStore {
    root: PathBuf,
}

impl ArtefactStore {
    /// Store under `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Store under `PROOF_ARTEFACT_ROOT`, else `/artefacts`.
    #[must_use]
    pub fn from_env() -> Self {
        let root = std::env::var(ARTEFACT_ROOT_ENV)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_ARTEFACT_ROOT.to_owned());
        Self::new(Path::new(&root))
    }

    /// Root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Highest numeric `pf_` id among zip basenames under this root.
    #[must_use]
    pub fn max_zip_numeric_id(&self) -> Option<u64> {
        max_zip_numeric_id(&self.root)
    }

    fn topic_dir(&self, topic_id: &str) -> Result<PathBuf, ArtefactError> {
        if !is_slug(topic_id) {
            return Err(ArtefactError::BadTopicId(topic_id.to_owned()));
        }
        let dir = self.root.join(topic_id);
        std::fs::create_dir_all(&dir).map_err(|e| ArtefactError::Io(e.to_string()))?;
        Ok(dir)
    }

    /// Write `{root}/{topic_id}/{submission_id}.zip`.
    ///
    /// # Errors
    ///
    /// Id / path validation, zip, or io failures. Nothing is written on error.
    pub fn write(
        &self,
        bundle: &ArtefactBundle,
        submission_id: &str,
        promoted: bool,
    ) -> Result<WrittenArtefact, ArtefactError> {
        let path = artefact_path(&self.root, &bundle.topic_id, submission_id)?;
        let bytes = bundle.zip_bytes(submission_id, promoted)?;
        self.topic_dir(&bundle.topic_id)?;
        std::fs::write(&path, &bytes).map_err(|e| ArtefactError::Io(e.to_string()))?;
        Ok(WrittenArtefact {
            path,
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.len() as u64,
        })
    }

    /// Write `{root}/{topic_id}/best.json`.
    ///
    /// # Errors
    ///
    /// Id validation or io failures.
    pub fn mark_best(&self, best: &BestRef) -> Result<PathBuf, ArtefactError> {
        artefact_path(&self.root, &best.topic_id, &best.submission_id)?;
        let path = self.topic_dir(&best.topic_id)?.join(BEST_FILE);
        let mut body = pretty(best);
        body.push(b'\n');
        std::fs::write(&path, body).map_err(|e| ArtefactError::Io(e.to_string()))?;
        Ok(path)
    }

    /// Read the current best pointer, if any.
    #[must_use]
    pub fn best(&self, topic_id: &str) -> Option<BestRef> {
        if !is_slug(topic_id) {
            return None;
        }
        let body = std::fs::read_to_string(self.root.join(topic_id).join(BEST_FILE)).ok()?;
        serde_json::from_str(&body).ok()
    }

    /// Append one public event to `{root}/{topic_id}/events.jsonl`.
    ///
    /// # Errors
    ///
    /// Id validation or io failures.
    pub fn append_event(&self, topic_id: &str, event: &PublicEvent) -> Result<(), ArtefactError> {
        let path = self.topic_dir(topic_id)?.join(EVENTS_FILE);
        let mut line =
            serde_json::to_string(event).map_err(|e| ArtefactError::Io(e.to_string()))?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ArtefactError::Io(e.to_string()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| ArtefactError::Io(e.to_string()))
    }

    /// Public events, oldest first (empty when none).
    #[must_use]
    pub fn events(&self, topic_id: &str) -> Vec<PublicEvent> {
        if !is_slug(topic_id) {
            return Vec::new();
        }
        std::fs::read_to_string(self.root.join(topic_id).join(EVENTS_FILE))
            .map(|body| {
                body.lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use proof_rlm::fixtures::{green, report_for, request, rules};

    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proof-rlm-artefacts-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn bundle(with_report: bool) -> ArtefactBundle {
        let req = request();
        let set = rules();
        let mut checklist = green(&set, &req.submission_digest);
        if !with_report {
            checklist.items[0].pass = false;
        }
        ArtefactBundle {
            topic_id: req.topic_id.clone(),
            custom_id: req.custom_id.clone(),
            submission_digest: req.submission_digest.clone(),
            artifact_digest: req.artifact_digest.clone(),
            checklist_green: with_report,
            checklist,
            report: with_report.then(|| report_for(&req, 0.6)),
            baseline_ref: BaselineRef {
                topic_id: req.topic_id.clone(),
                script_sha256: "11".repeat(32),
                metrics_commitment: "22".repeat(32),
                holdout_commitment: "33".repeat(32),
                eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            },
            artifact: vec![
                ArtifactFile {
                    path: "src/main.rs".into(),
                    bytes: b"fn main() {}\n".to_vec(),
                },
                ArtifactFile {
                    path: "Cargo.toml".into(),
                    bytes: b"[package]\n".to_vec(),
                },
            ],
            logs: vec![LogFile {
                name: "run.log".into(),
                bytes: b"ok\n".to_vec(),
            }],
        }
    }

    fn entries_of(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("zip");
        let mut out = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).expect("entry");
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).expect("read");
            out.push((f.name().to_owned(), buf));
        }
        out
    }

    #[test]
    fn the_zip_has_the_documented_layout_and_is_deterministic() {
        let b = bundle(true);
        let a = b.zip_bytes("pf_0000000000000001", false).expect("zip");
        let again = b.zip_bytes("pf_0000000000000001", false).expect("zip");
        assert_eq!(a, again, "same bundle, same bytes");
        let entries = entries_of(&a);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "artifact/Cargo.toml",
                "artifact/src/main.rs",
                "baseline_ref.json",
                "checklist.json",
                "logs/run.log",
                "manifest.json",
                "report.json",
            ]
        );
        let manifest: ArtefactManifest = serde_json::from_slice(&entries[5].1).expect("manifest");
        assert_eq!(manifest.submission_id, "pf_0000000000000001");
        assert_eq!(manifest.rules_version, 1);
        assert!((manifest.primary_value.expect("primary") - 0.6).abs() < 1e-12);
        assert!(manifest.checklist_green);
        assert!(!manifest.promoted);
        assert_eq!(manifest.entries, names);
        let report = CustomRunReport::from_json(std::str::from_utf8(&entries[6].1).expect("utf8"))
            .expect("report");
        assert_eq!(report.topic_id, b.topic_id);
    }

    /// A pre-spend reject still ships a bundle: the red checklist is the
    /// evidence, and there is no report to ship.
    #[test]
    fn a_red_checklist_bundle_has_no_report() {
        let b = bundle(false);
        let bytes = b.zip_bytes("pf_0000000000000002", false).expect("zip");
        let names: Vec<String> = entries_of(&bytes).into_iter().map(|(n, _)| n).collect();
        assert!(!names.iter().any(|n| n == REPORT_FILE), "{names:?}");
        let m = b.manifest("pf_0000000000000002", false);
        assert!(!m.checklist_green);
        assert_eq!(m.primary_value, None);
    }

    #[test]
    fn entry_paths_cannot_escape_or_collide() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a//b",
            "trailing/",
            "sp ace",
            "back\\slash",
            &"x".repeat(MAX_ENTRY_PATH + 1),
        ] {
            assert!(!is_safe_entry_path(bad), "{bad:?}");
        }
        for good in ["Cargo.toml", "src/main.rs", ".gitignore", "a-b_c.d/e"] {
            assert!(is_safe_entry_path(good), "{good:?}");
        }
        let mut b = bundle(true);
        b.artifact.push(ArtifactFile {
            path: "../escape".into(),
            bytes: Vec::new(),
        });
        assert_eq!(
            b.zip_bytes("pf_0000000000000003", false),
            Err(ArtefactError::BadEntryPath("../escape".into()))
        );
        let mut nested_log = bundle(true);
        nested_log.logs[0].name = "a/b.log".into();
        assert!(matches!(
            nested_log.zip_bytes("pf_0000000000000003", false),
            Err(ArtefactError::BadEntryPath(_))
        ));
        let mut dup = bundle(true);
        dup.artifact.push(ArtifactFile {
            path: "Cargo.toml".into(),
            bytes: b"again".to_vec(),
        });
        assert!(matches!(
            dup.zip_bytes("pf_0000000000000003", false),
            Err(ArtefactError::BadEntryPath(_))
        ));
    }

    #[test]
    fn ids_are_validated_before_touching_the_filesystem() {
        let root = Path::new("/artefacts");
        assert_eq!(
            artefact_path(root, "topic-a", "pf_000000000000002a").expect("path"),
            PathBuf::from("/artefacts/topic-a/pf_000000000000002a.zip")
        );
        assert!(matches!(
            artefact_path(root, "../etc", "pf_0000000000000001"),
            Err(ArtefactError::BadTopicId(_))
        ));
        assert!(matches!(
            artefact_path(root, "topic-a", "../../x"),
            Err(ArtefactError::BadSubmissionId(_))
        ));
        assert!(matches!(
            artefact_path(root, "topic-a", "pf_1"),
            Err(ArtefactError::BadSubmissionId(_))
        ));
    }

    #[test]
    fn the_store_writes_zips_the_best_pointer_and_public_events() {
        let root = tmp("store");
        let store = ArtefactStore::new(&root);
        assert_eq!(store.root(), root.as_path());
        let b = bundle(true);
        let written = store.write(&b, "pf_0000000000000007", true).expect("write");
        assert_eq!(
            written.path,
            root.join("topic-a").join("pf_0000000000000007.zip")
        );
        let bytes = std::fs::read(&written.path).expect("read");
        assert_eq!(
            bytes,
            b.zip_bytes("pf_0000000000000007", true).expect("zip")
        );
        assert_eq!(written.sha256, hex::encode(Sha256::digest(&bytes)));
        assert_eq!(written.bytes, bytes.len() as u64);
        assert!(store.best("topic-a").is_none());
        let pointer = store
            .mark_best(&BestRef {
                topic_id: "topic-a".into(),
                submission_id: "pf_0000000000000007".into(),
                submission_digest: b.submission_digest.clone(),
                primary_value: 0.6,
                bar: Some(0.5),
                artefact: "pf_0000000000000007.zip".into(),
            })
            .expect("pointer");
        assert_eq!(pointer, root.join("topic-a").join(BEST_FILE));
        assert_eq!(
            store.best("topic-a").expect("best").submission_id,
            "pf_0000000000000007"
        );
        assert!(store.best("../x").is_none());
        store
            .append_event(
                "topic-a",
                &PublicEvent::Scored {
                    submission_id: "pf_0000000000000007".into(),
                    primary_value: Some(0.6),
                    checklist_green: true,
                },
            )
            .expect("event");
        store
            .append_event(
                "topic-a",
                &PublicEvent::Promoted {
                    submission_id: "pf_0000000000000007".into(),
                    primary_value: 0.6,
                    bar: Some(0.5),
                    previous_best: None,
                },
            )
            .expect("event");
        let events = store.events("topic-a");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1], PublicEvent::Promoted { .. }));
        assert!(store.events("topic-b").is_empty());
        assert!(store.append_event("Bad Topic", &events[0]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_root_comes_from_the_env_or_the_default() {
        assert_eq!(
            ArtefactStore::new(Path::new(DEFAULT_ARTEFACT_ROOT)).root(),
            Path::new("/artefacts")
        );
        assert_eq!(ARTEFACT_ROOT_ENV, "PROOF_ARTEFACT_ROOT");
    }

    #[test]
    fn max_zip_numeric_id_reads_pf_basenames_across_topic_dirs() {
        let root = tmp("max-zip");
        std::fs::create_dir_all(root.join("topic-a")).expect("a");
        std::fs::create_dir_all(root.join("topic-b")).expect("b");
        std::fs::write(root.join("topic-a").join("pf_0000000000000001.zip"), b"a").expect("zip 1");
        std::fs::write(root.join("topic-b").join("pf_00000000000000ff.zip"), b"b").expect("zip ff");
        std::fs::write(root.join("topic-a").join("best.json"), b"{}").expect("best");
        std::fs::write(root.join("not-a-topic.zip"), b"x").expect("root zip");
        assert_eq!(max_zip_numeric_id(&root), Some(0xff));
        assert_eq!(ArtefactStore::new(&root).max_zip_numeric_id(), Some(0xff));
        assert_eq!(max_zip_numeric_id(&root.join("missing")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
