//! base workspace maintenance binary.
//!
//! Subcommands:
//! - `loc-cap` — fail if any crate under `crates/` or `bins/` exceeds 1500 non-test LOC
//! - `consensus-lint` — fail if listed consensus crates use forbidden tokens (D8)
//! - `metadata-snapshot` — fetch testnet metadata + epoch-schedule sources into `metadata/testnet.lock`
//! - `natural-pack` — build the pinned G5 natural-document eval packs into the operator assets dir
//! - `spec-check` — fail if `docs/BUNDLE_SPEC.md` is missing plan pins (a)–(l)
//! - `design-check` — fail if `docs/DESIGN_CHALLENGE.md` is missing freeze pins
//! - `external-docs-check` — fail if external miner docs `protocol_version` ≠ bundle, or D19 drifts
//! - `relearn-t2i-holdout` — select a Relearn T2I holdout slice and print its commitment
//! - `relearn-holdout` — select a Relearn holdout slice and print its commitment
//! - `relearn-agent-holdout` — select a Relearn Agent episode set and print its commitment
//! - `proof-holdout` — select a Proof per-topic holdout set and print its commitment
//! - `proof-topic` — sign a Proof topic document with the `proof` row key
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod agent_holdout;
mod consensus_lint;
mod design_check;
mod external_docs_check;
mod loc_cap;
mod metadata_snapshot;
mod natural_pack;
mod proof_holdout;
mod proof_topic;
mod relearn_holdout;
mod spec_check;
mod t2i_holdout;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "base workspace maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Fail if any package under crates/ or bins/ exceeds 1500 non-test Rust LOC.
    LocCap,
    /// Fail if consensus crates contain `HashMap`, `f32`/`f64`, `wrapping_*`, or bare `u128` ops.
    ConsensusLint,
    /// Snapshot Finney testnet metadata + epoch-schedule read paths into a lockfile.
    MetadataSnapshot {
        /// JSON-RPC endpoint (`wss://` is rewritten to `https://`).
        #[arg(long, default_value = metadata_snapshot::DEFAULT_ENDPOINT)]
        endpoint: String,
        /// Netuid used when probing per-subnet schedule storage (default 1).
        #[arg(long, default_value_t = metadata_snapshot::DEFAULT_SNAPSHOT_NETUID)]
        netuid: u16,
        /// Lockfile path relative to workspace root (or absolute).
        #[arg(long, default_value = "metadata/testnet.lock")]
        out: PathBuf,
        /// Compare live snapshot to the committed lockfile; exit 1 on drift.
        #[arg(long)]
        check: bool,
    },
    /// Build the pinned G5 natural-document eval packs (LongBench-v2 MCQ + HELMET RAG).
    NaturalPack {
        /// Operator eval-assets root; packs land under `<out>/g5/natural/`.
        #[arg(long, default_value = "prism-eval-assets")]
        out: PathBuf,
        /// Source artifact cache (downloads + extracted archive members).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Pack seed; changing it rotates which rows are private vs mirror.
        #[arg(long, default_value = natural_pack::DEFAULT_PACK_SEED)]
        seed: String,
        /// MCQ rows per side (private and mirror each get this many).
        #[arg(long, default_value_t = 64)]
        mcq_pool: usize,
        /// RAG rows per side, per (corpus, k) cell.
        #[arg(long, default_value_t = 12)]
        rag_per_cell: usize,
        /// Few-shot demo rows per side, per corpus.
        #[arg(long, default_value_t = 8)]
        demos_per_corpus: usize,
        /// LongBench-v2 length bands to draw from (repeatable).
        #[arg(long = "length", default_values_t = [String::from("short")])]
        lengths: Vec<String>,
        /// Never touch the network; every artifact must already be cached.
        #[arg(long)]
        offline: bool,
        /// Rebuild beside `<out>` and fail if the pack hash drifted.
        #[arg(long)]
        check: bool,
    },
    /// Fail if `docs/BUNDLE_SPEC.md` is missing required (a)–(l) pins (task 8).
    SpecCheck,
    /// Fail if `docs/DESIGN_CHALLENGE.md` is missing required freeze pins.
    DesignCheck,
    /// Fail if external miner docs `protocol_version` differs from `bundle`, or `THREAT_MODEL` D19 drifts.
    ExternalDocsCheck,
    /// Select a Relearn holdout slice and print its pin commitment.
    ///
    /// Item ids are never printed. Production salts stay off git. Refuses the
    /// documented T2I/dev salt.
    RelearnHoldout {
        /// Operator catalog JSON (array of holdout items).
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Selection salt. Never reuse the T2I/dev salt.
        #[arg(long)]
        salt: String,
        /// Holdout item count.
        #[arg(long, default_value_t = 120)]
        size: usize,
        /// Item ids published in the pin's public split (repeatable).
        #[arg(long = "exclude")]
        exclude: Vec<u32>,
        /// Build a local-only synthetic catalog.
        #[arg(long)]
        synthetic: bool,
        /// Write records here (outside the repo, mode 0600).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Select a Relearn Agent episode set and print its pin commitment.
    ///
    /// Episode ids and goals are never printed. Production salts stay off git.
    RelearnAgentHoldout {
        /// Operator catalogue JSON (array of episodes).
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Selection salt. Never reuse another challenge's salt.
        #[arg(long)]
        salt: String,
        /// Holdout episode count.
        #[arg(long, default_value_t = 120)]
        size: usize,
        /// Episode ids published in the pin's public split (repeatable).
        #[arg(long = "exclude")]
        exclude: Vec<u32>,
        /// Build a local-only synthetic catalogue.
        #[arg(long)]
        synthetic: bool,
        /// Write episodes here (outside the repo, mode 0600).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Select a Relearn T2I holdout slice and print its pin commitment.
    ///
    /// Prompt ids are never printed. Production salts stay off git.
    RelearnT2iHoldout {
        /// Bench prompt file (`qwen_image_bench_hf_v0518.jsonl`).
        #[arg(long)]
        bench: PathBuf,
        /// Selection salt.
        #[arg(long)]
        salt: String,
        /// Holdout prompt count.
        #[arg(long, default_value_t = 40)]
        size: usize,
        /// Prompt ids published in the pin's public split (repeatable).
        #[arg(long = "exclude")]
        exclude: Vec<u32>,
        /// Write records here (outside the repo, mode 0600).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Select a Proof per-topic holdout set and print its commitment.
    ///
    /// Records are never printed. Production salts stay off git. Refuses
    /// other challenges' documented salts and writes under config/ or docs/.
    ProofHoldout {
        /// Topic id these records will be scored under.
        #[arg(long)]
        topic_id: String,
        /// Operator catalog JSON (array of holdout records).
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Selection salt. Never reuse another challenge's salt.
        #[arg(long)]
        salt: String,
        /// Holdout record count (must stay stratified).
        #[arg(long, default_value_t = 120)]
        size: usize,
        /// Record ids to exclude (repeatable).
        #[arg(long = "exclude")]
        exclude: Vec<u32>,
        /// Build a local-only synthetic catalog.
        #[arg(long)]
        synthetic: bool,
        /// Write records here (outside the repo, mode 0600).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Sign a Proof topic document with the `proof` row mini-secret.
    ProofTopic {
        /// Unsigned (or previously signed) topic JSON.
        #[arg(long)]
        input: PathBuf,
        /// Mini-secret file (raw 32 bytes or hex). Never commit.
        #[arg(long)]
        secret: PathBuf,
        /// Write signed JSON here (outside the repo). Stdout when omitted.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask crate must live one level under the workspace root".into())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = match workspace_root() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("xtask error: {err}");
            return ExitCode::FAILURE;
        }
    };
    match dispatch(cli.command, &root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)] // one arm per subcommand; splitting hides the map
fn dispatch(command: Command, root: &Path) -> Result<(), String> {
    match command {
        Command::LocCap => loc_cap::run(root),
        Command::ConsensusLint => consensus_lint::run(root),
        Command::MetadataSnapshot {
            endpoint,
            netuid,
            out,
            check,
        } => {
            let args = metadata_snapshot::SnapshotArgs {
                endpoint,
                netuid,
                out,
                check,
            };
            metadata_snapshot::run(root, &args)
        }
        Command::NaturalPack {
            out,
            cache,
            seed,
            mcq_pool,
            rag_per_cell,
            demos_per_corpus,
            lengths,
            offline,
            check,
        } => {
            let defaults = natural_pack::PackArgs::default();
            let args = natural_pack::PackArgs {
                out,
                cache: cache.unwrap_or(defaults.cache),
                seed,
                mcq_pool,
                rag_per_cell,
                demos_per_corpus,
                lengths,
                offline,
                check,
            };
            natural_pack::run(root, &args)
        }
        Command::SpecCheck => spec_check::run(root),
        Command::DesignCheck => design_check::run(root),
        Command::ExternalDocsCheck => external_docs_check::run(root),
        Command::RelearnHoldout {
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        } => relearn_holdout::run(&relearn_holdout::HoldoutArgs {
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        }),
        Command::RelearnAgentHoldout {
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        } => agent_holdout::run(&agent_holdout::HoldoutArgs {
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        }),
        Command::RelearnT2iHoldout {
            bench,
            salt,
            size,
            exclude,
            out,
        } => t2i_holdout::run(&t2i_holdout::HoldoutArgs {
            bench,
            salt,
            size,
            exclude,
            out,
        }),
        Command::ProofHoldout {
            topic_id,
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        } => proof_holdout::run(&proof_holdout::HoldoutArgs {
            topic_id,
            catalog,
            salt,
            size,
            exclude,
            synthetic,
            out,
        }),
        Command::ProofTopic { input, secret, out } => {
            proof_topic::run(&proof_topic::TopicArgs { input, secret, out })
        }
    }
}
