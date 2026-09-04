//! `ctx` — the Cortex subnet CLI.
//!
//! One binary for the two live challenges: read what a host can score, submit
//! a Proof experiment against an open `topic_id`, and pair plus file reports
//! for Bounty. It talks to the public gateway over HTTPS and signs Bounty
//! pairings locally; it never asks for a mnemonic and never prints a key.

#![forbid(unsafe_code)]

mod api;
mod bounty;
mod catalog;
mod off;
mod proof;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use api::{Client, DEFAULT_GATEWAY};
use bounty::Signer;
use proof::SubmitInput;

/// Cortex subnet CLI.
#[derive(Debug, Parser)]
#[command(
    name = "ctx",
    version,
    about = "Cortex subnet CLI: challenge status, Proof submits, and Bounty reports",
    long_about = "ctx talks to the public Cortex gateway at https://network.cortex.foundation.

Live challenges: bounty (2000 bps) and proof (8000 bps). relearn*, design, and
prism are off and earn nothing. `ctx relearn|image|agent` still exist for a
local stack; they are not live work.

Start with 'ctx challenges' for what each one pays for, then 'ctx status' to
see whether a host can score right now. A challenge that cannot score answers
503 on submit rather than banking work it could never pay for."
)]
struct Cli {
    /// Gateway base URL. Change this only when you run your own stack.
    #[arg(long, global = true, default_value = DEFAULT_GATEWAY, value_name = "URL")]
    gateway: String,
    /// Print raw JSON replies instead of a summary.
    #[arg(long, global = true)]
    json: bool,
    /// Miner-pays-Lium key, forwarded as X-Lium-Api-Key and never printed.
    #[arg(
        long,
        global = true,
        env = "LIUM_API_KEY",
        hide_env_values = true,
        value_name = "KEY"
    )]
    lium_api_key: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Show every live challenge, whether it can score, and the sealed vector.
    Status {
        /// Limit to one challenge id.
        #[arg(long, value_name = "ID")]
        challenge: Option<String>,
    },
    /// List the live challenges, their emission, and what they pay for.
    Challenges,
    /// Show the latest sealed weights vector (or the fail-closed burn vector).
    Weights,
    /// Proof: reproduce an operator-published research topic.
    Proof {
        #[command(subcommand)]
        cmd: ProofCmd,
    },
    /// Bounty: pair a hotkey, then file real bug reports.
    Bounty {
        #[command(subcommand)]
        cmd: BountyCmd,
    },
    /// Relearn (off): no emission. Local-stack submit only.
    Relearn {
        #[command(subcommand)]
        cmd: off::OffCmd,
    },
    /// Relearn Image (off): no emission. Local-stack submit only.
    Image {
        #[command(subcommand)]
        cmd: off::ImageCmd,
    },
    /// Relearn Agent (off): no emission. Local-stack submit only.
    Agent {
        #[command(subcommand)]
        cmd: off::OffCmd,
    },
}

#[derive(Debug, Subcommand)]
enum ProofCmd {
    /// Submit an artifact digest against an open `topic_id`.
    Submit(Box<ProofSubmitArgs>),
    /// Show one submission.
    Show {
        /// Submission id returned by submit.
        id: String,
        /// Keep polling until the submission stops moving.
        #[arg(long)]
        wait: bool,
    },
    /// Show this challenge's live status.
    Status,
    /// List currently published topics (never holdout records).
    Topics,
}

#[derive(Debug, Subcommand)]
enum BountyCmd {
    /// Bind a hotkey to a dedicated Cortex Chat mining account.
    Pair {
        /// Miner hotkey, SS58 or 64-hex.
        #[arg(long, value_name = "HOTKEY")]
        hotkey: String,
        /// Dedicated Cortex Chat mining account id. Never a personal account.
        #[arg(long, value_name = "ACCOUNT")]
        account_id: String,
        /// Accept the pairing terms. Pairing is blocking without this.
        #[arg(long)]
        accept_terms: bool,
        /// 32-byte hotkey mini-secret file. Never a mnemonic.
        #[arg(long, value_name = "PATH")]
        secret_file: Option<PathBuf>,
        /// Bittensor wallets directory.
        #[arg(long, value_name = "PATH")]
        wallet_dir: Option<PathBuf>,
        /// Wallet name to sign with.
        #[arg(long, value_name = "NAME")]
        wallet_name: Option<String>,
        /// Hotkey file inside that wallet.
        #[arg(long, default_value = "default", value_name = "NAME")]
        wallet_hotkey: String,
        /// 128-hex signature produced by an offline sign.
        #[arg(long, value_name = "HEX")]
        signature: Option<String>,
    },
    /// File a bug report against the paired hotkey.
    Report {
        /// Short bug title.
        #[arg(long, value_name = "TEXT")]
        title: String,
        /// Report body: what broke.
        #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the report body from a file.
        #[arg(long, value_name = "PATH")]
        body_file: Option<PathBuf>,
        /// How to reproduce it.
        #[arg(long, value_name = "TEXT", conflicts_with = "repro_file")]
        repro: Option<String>,
        /// Read the reproduction steps from a file.
        #[arg(long, value_name = "PATH")]
        repro_file: Option<PathBuf>,
        /// Session claim. Defaults to the one cached by pair.
        #[arg(long, value_name = "CLAIM")]
        session: Option<String>,
    },
    /// Report bodies are operator-local (not on the public gateway).
    Show {
        /// Report id returned by report.
        id: String,
    },
    /// Show bounty quotas and whether this host can score.
    Status,
}

/// Artifact and topic arguments for a Proof submit.
#[derive(Debug, Args)]
struct ProofSubmitArgs {
    /// 64-hex miner hotkey.
    #[arg(long, value_name = "HEX64")]
    hotkey: String,
    /// Open topic id (`ctx proof topics`).
    #[arg(long, value_name = "ID")]
    topic_id: String,
    /// SHA-256 hex of the artifact you are submitting.
    #[arg(long, value_name = "SHA256")]
    artifact_digest: String,
    /// Optional locator for the artifact (git url, object URL).
    #[arg(long, value_name = "URL")]
    artifact_uri: Option<String>,
    /// Architecture / proxy id baked by the pin.
    #[arg(long, value_name = "ID")]
    architecture: String,
    /// What the recipe achieved (the RLM re-runs this claim).
    #[arg(long, value_name = "TEXT")]
    claim: String,
    /// FLOPs spent reproducing the recipe. Must be ≤ the topic budget.
    #[arg(long, value_name = "N")]
    declared_flops: u64,
    /// Full manifest JSON file, used verbatim.
    #[arg(long, value_name = "PATH")]
    manifest_file: Option<PathBuf>,
    /// Shard content hash you trained on (repeatable).
    #[arg(long = "train-hash", value_name = "SHA256")]
    train_hashes: Vec<String>,
    /// Dataset / corpus id you trained on (repeatable).
    #[arg(long = "train-dataset", value_name = "ID")]
    train_datasets: Vec<String>,
    /// Keep polling until the submission stops moving.
    #[arg(long)]
    wait: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let client = Client::new(&cli.gateway, cli.lium_api_key)?;
    match cli.cmd {
        Cmd::Status { challenge } => {
            catalog::print_status(&client, challenge.as_deref(), cli.json).await
        }
        Cmd::Challenges => {
            catalog::print_challenges();
            Ok(())
        }
        Cmd::Weights => catalog::print_weights(&client, cli.json).await,
        Cmd::Proof { cmd } => run_proof(&client, cmd, cli.json).await,
        Cmd::Bounty { cmd } => run_bounty(&client, cmd, cli.json).await,
        Cmd::Relearn { cmd } => off::run(&client, "relearn", cmd, cli.json).await,
        Cmd::Image { cmd } => off::run_image(&client, cmd, cli.json).await,
        Cmd::Agent { cmd } => off::run(&client, "relearn-agent", cmd, cli.json).await,
    }
}

async fn run_proof(client: &Client, cmd: ProofCmd, json: bool) -> Result<(), String> {
    match cmd {
        ProofCmd::Submit(args) => {
            let input = SubmitInput {
                hotkey: args.hotkey,
                topic_id: args.topic_id,
                artifact_digest: args.artifact_digest,
                artifact_uri: args.artifact_uri,
                architecture: args.architecture,
                claim: args.claim,
                declared_flops: args.declared_flops,
                manifest_file: args.manifest_file,
                train_hashes: args.train_hashes,
                train_datasets: args.train_datasets,
                wait: args.wait,
            };
            proof::submit(client, &input, json).await
        }
        ProofCmd::Show { id, wait } => proof::show(client, &id, wait, json).await,
        ProofCmd::Status => catalog::print_status(client, Some("proof"), json).await,
        ProofCmd::Topics => proof::topics(client, json).await,
    }
}

async fn run_bounty(client: &Client, cmd: BountyCmd, json: bool) -> Result<(), String> {
    match cmd {
        BountyCmd::Pair {
            hotkey,
            account_id,
            accept_terms,
            secret_file,
            wallet_dir,
            wallet_name,
            wallet_hotkey,
            signature,
        } => {
            let signer = Signer {
                secret_file,
                wallet_dir,
                wallet_name,
                wallet_hotkey,
                signature,
            };
            bounty::pair(client, &hotkey, &account_id, &signer, accept_terms, json).await
        }
        BountyCmd::Report {
            title,
            body,
            body_file,
            repro,
            repro_file,
            session,
        } => {
            let body_text = text_arg(body, body_file.as_deref(), "--body")?;
            let repro_text = text_arg(repro, repro_file.as_deref(), "--repro")?;
            bounty::report(
                client,
                &title,
                &body_text,
                &repro_text,
                session.as_deref(),
                json,
            )
            .await
        }
        BountyCmd::Show { id } => bounty::show(&id, json),
        BountyCmd::Status => bounty::status(client, json).await,
    }
}

fn text_arg(
    inline: Option<String>,
    file: Option<&std::path::Path>,
    what: &str,
) -> Result<String, String> {
    match (inline, file) {
        (Some(text), _) => Ok(text),
        (None, Some(path)) => {
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
        }
        (None, None) => Err(format!("{what} is required")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn default_gateway_matches_the_docs_host() {
        assert_eq!(DEFAULT_GATEWAY, "https://network.cortex.foundation");
    }

    #[test]
    fn live_catalog_is_bounty_and_proof() {
        let ids: Vec<&str> = catalog::LIVE.iter().map(|c| c.id).collect();
        assert_eq!(ids, ["bounty", "proof"]);
        let sum: u32 = catalog::LIVE.iter().map(|c| c.emission_bps).sum();
        assert_eq!(sum, 10_000);
    }

    #[test]
    fn off_commands_parse_but_are_not_live() {
        assert!(Cli::try_parse_from(["ctx", "relearn", "status"]).is_ok());
        assert!(Cli::try_parse_from(["ctx", "image", "prompts"]).is_ok());
        assert!(Cli::try_parse_from(["ctx", "agent", "status"]).is_ok());
        assert!(Cli::try_parse_from(["ctx", "relearn", "prompts"]).is_err());
        assert!(catalog::find("relearn").is_none());
        assert_eq!(
            catalog::find_off("relearn").map(|c| c.emission_bps),
            Some(0)
        );
    }

    #[test]
    fn off_submit_parses_repeatable_manifest_flags() {
        let cli = Cli::try_parse_from([
            "ctx",
            "relearn",
            "submit",
            "--hotkey",
            &"ab".repeat(32),
            "--artifact-digest",
            &"cd".repeat(32),
            "--train-id",
            "1",
            "--train-dataset",
            "my-mix",
        ])
        .expect("parse");
        match cli.cmd {
            Cmd::Relearn {
                cmd: off::OffCmd::Submit(args),
            } => {
                assert_eq!(args.train_ids, vec![1]);
                assert_eq!(args.train_datasets, vec!["my-mix".to_owned()]);
            }
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn bounty_pair_terms_flag_is_opt_in() {
        let cli = Cli::try_parse_from([
            "ctx",
            "bounty",
            "pair",
            "--hotkey",
            "5F3sa2TJAWMqDhXG6jhV4N8ko9SxwGy8TpaNS1repo5EYjQX",
            "--account-id",
            "acct-miner-1",
        ])
        .expect("parse");
        match cli.cmd {
            Cmd::Bounty {
                cmd: BountyCmd::Pair { accept_terms, .. },
            } => assert!(!accept_terms),
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn help_text_names_the_public_gateway_and_no_placeholder_host() {
        let long_about = Cli::command()
            .get_long_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            long_about.contains("https://network.cortex.foundation"),
            "{long_about}"
        );
        assert!(!long_about.contains("<gateway>"), "{long_about}");
        assert!(long_about.contains("bounty (2000 bps)"));
        assert!(long_about.contains("proof (8000 bps)"));
    }

    #[test]
    fn text_arg_prefers_inline_then_file() {
        assert_eq!(
            text_arg(Some("inline".into()), None, "--body").expect("inline"),
            "inline"
        );
        assert!(text_arg(None, None, "--body").is_err());
    }
}
