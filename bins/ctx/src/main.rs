//! `ctx` — the Cortex subnet CLI.
//!
//! One binary for the four live challenges: read what a host can score, submit
//! to Relearn / Relearn Image / Relearn Agent, and pair plus file reports for
//! Bounty. It talks to the public gateway over HTTPS and signs Bounty pairings
//! locally; it never asks for a mnemonic and never prints a key.

#![forbid(unsafe_code)]

mod api;
mod bounty;
mod catalog;
mod submit;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::Value;

use api::{Client, DEFAULT_GATEWAY};
use submit::SubmitInput;

/// Cortex subnet CLI.
#[derive(Debug, Parser)]
#[command(
    name = "ctx",
    version,
    about = "Cortex subnet CLI: challenge status, submits, and Bounty reports",
    long_about = "ctx talks to the public Cortex gateway at https://network.cortex.foundation.

Live challenges: relearn (4000 bps), relearn-image (1500), relearn-agent
(1500), bounty (3000). relearn-mm is off and earns nothing.

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
    /// Relearn: post-train Qwen/Qwen3.8-27B.
    Relearn {
        #[command(subcommand)]
        cmd: ChallengeCmd,
    },
    /// Relearn Image: fine-tune the pinned Cosmos3 generator.
    Image {
        #[command(subcommand)]
        cmd: ImageCmd,
    },
    /// Relearn Agent: post-train a tool-using agent scored on replayed traces.
    Agent {
        #[command(subcommand)]
        cmd: ChallengeCmd,
    },
    /// Bounty: pair a hotkey, then file real bug reports.
    Bounty {
        #[command(subcommand)]
        cmd: BountyCmd,
    },
}

#[derive(Debug, Subcommand)]
enum ChallengeCmd {
    /// Submit an artifact digest plus the manifest of what you trained on.
    Submit(Box<SubmitArgs>),
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
}

#[derive(Debug, Subcommand)]
enum ImageCmd {
    /// Submit an artifact digest plus the manifest of what you trained on.
    Submit(Box<SubmitArgs>),
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
    /// Show the frozen public prompt split and its seeds.
    Prompts,
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
    /// Show one filed report.
    Show {
        /// Report id returned by report.
        id: String,
    },
    /// Show bounty quotas and whether this host can score.
    Status,
}

/// Artifact and manifest arguments for a Relearn-family submit.
#[derive(Debug, Args)]
struct SubmitArgs {
    /// 64-hex miner hotkey.
    #[arg(long, value_name = "HEX64")]
    hotkey: String,
    /// SHA-256 hex of the artifact you are submitting.
    #[arg(long, value_name = "SHA256")]
    artifact_digest: String,
    /// Optional locator for the artifact (HF repo, object URL).
    #[arg(long, value_name = "URL")]
    artifact_uri: Option<String>,
    /// Full manifest JSON file, used verbatim.
    #[arg(long, value_name = "PATH")]
    manifest_file: Option<PathBuf>,
    /// Public item, prompt, or episode id your training mix touched. Repeatable.
    #[arg(long = "train-id", value_name = "ID")]
    train_ids: Vec<u32>,
    /// Image or observation hash your training mix touched. Repeatable.
    #[arg(long = "train-hash", value_name = "SHA256")]
    train_hashes: Vec<String>,
    /// Dataset, corpus, or environment id you trained on. Repeatable.
    #[arg(long = "train-dataset", value_name = "ID")]
    train_datasets: Vec<String>,
    /// Declared base checkpoint. Relearn Image only.
    #[arg(long, value_name = "MODEL")]
    base: Option<String>,
    /// Declared base license. Relearn Image only.
    #[arg(long, value_name = "LICENSE")]
    base_license: Option<String>,
    /// Seed-replay claim as cell=sha256, e.g. p1#v0=abc… Relearn Image only.
    #[arg(long = "claimed-output", value_name = "CELL=SHA256")]
    claimed_outputs: Vec<String>,
    /// Poll the submission until it stops moving.
    #[arg(long)]
    wait: bool,
}

impl From<SubmitArgs> for SubmitInput {
    fn from(a: SubmitArgs) -> Self {
        Self {
            hotkey: a.hotkey,
            artifact_digest: a.artifact_digest,
            artifact_uri: a.artifact_uri,
            manifest_file: a.manifest_file,
            train_ids: a.train_ids,
            train_hashes: a.train_hashes,
            train_datasets: a.train_datasets,
            base: a.base,
            base_license: a.base_license,
            claimed_outputs: a.claimed_outputs,
            wait: a.wait,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ctx: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let json = cli.json;
    let client = Client::new(&cli.gateway, cli.lium_api_key)?;
    match cli.cmd {
        Cmd::Challenges => {
            catalog::print_challenges();
            Ok(())
        }
        Cmd::Status { challenge } => {
            let only = match challenge.as_deref() {
                Some(name) => Some(resolve(name)?.id),
                None => None,
            };
            catalog::print_status(&client, only, json).await
        }
        Cmd::Weights => catalog::print_weights(&client, json).await,
        Cmd::Relearn { cmd } => challenge_cmd(&client, "relearn", cmd, json).await,
        Cmd::Agent { cmd } => challenge_cmd(&client, "relearn-agent", cmd, json).await,
        Cmd::Image { cmd } => image_cmd(&client, cmd, json).await,
        Cmd::Bounty { cmd } => bounty_cmd(&client, cmd, json).await,
    }
}

async fn challenge_cmd(
    client: &Client,
    id: &str,
    cmd: ChallengeCmd,
    json: bool,
) -> Result<(), String> {
    let challenge = resolve(id)?;
    match cmd {
        ChallengeCmd::Submit(args) => {
            submit::submit(client, challenge, &SubmitInput::from(*args), json).await
        }
        ChallengeCmd::Show { id, wait } => submit::show(client, challenge, &id, wait, json).await,
        ChallengeCmd::Status => print_one_status(client, id, json).await,
    }
}

async fn image_cmd(client: &Client, cmd: ImageCmd, json: bool) -> Result<(), String> {
    let challenge = resolve("relearn-image")?;
    match cmd {
        ImageCmd::Submit(args) => {
            submit::submit(client, challenge, &SubmitInput::from(*args), json).await
        }
        ImageCmd::Show { id, wait } => submit::show(client, challenge, &id, wait, json).await,
        ImageCmd::Status => print_one_status(client, challenge.id, json).await,
        ImageCmd::Prompts => print_prompts(client, json).await,
    }
}

async fn bounty_cmd(client: &Client, cmd: BountyCmd, json: bool) -> Result<(), String> {
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
            let signer = bounty::Signer {
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
            let body = text_arg(body, body_file.as_deref(), "--body / --body-file")?;
            let repro = text_arg(repro, repro_file.as_deref(), "--repro / --repro-file")?;
            bounty::report(client, &title, &body, &repro, session.as_deref(), json).await
        }
        BountyCmd::Show { id } => bounty::show(client, &id, json).await,
        BountyCmd::Status => bounty::status(client, json).await,
    }
}

async fn print_one_status(client: &Client, id: &str, json: bool) -> Result<(), String> {
    let challenge = resolve(id)?;
    let body = catalog::fetch_status(client, challenge.id).await?;
    if json {
        println!("{body}");
    } else {
        println!("{} ({})", challenge.label, challenge.id);
        print_status_body(&body);
    }
    Ok(())
}

fn print_status_body(body: &Value) {
    if let Some(map) = body.as_object() {
        for (k, v) in map {
            println!("  {k}: {}", catalog::compact(v));
        }
    }
}

async fn print_prompts(client: &Client, json: bool) -> Result<(), String> {
    let reply = client.get("/challenge/relearn-image/v1/prompts").await?;
    if json {
        println!("{}", reply.body);
        return Ok(());
    }
    if !reply.ok() {
        return Err(format!("HTTP {}: {}", reply.status, reply.message()));
    }
    let cells = reply
        .body
        .get("public")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!("public cells: {cells}");
    if let Some(v) = reply.body.get("dataset") {
        println!("dataset: {}", catalog::compact(v));
    }
    if let Some(v) = reply.body.get("holdout") {
        println!("holdout commitment: {v}");
    }
    println!();
    println!("These strings and seeds are frozen: every miner generates the same");
    println!("cells at the same seeds, so the images are comparable. You do not bring");
    println!("an upsampler to the scored split, and the holdout prompts are never");
    println!("published — only their commitment and size.");
    println!();
    println!("Full cells (prompt text, seed, cell key):");
    println!("  ctx image prompts --json");
    Ok(())
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

fn resolve(name: &str) -> Result<&'static catalog::Challenge, String> {
    catalog::find(name).ok_or_else(|| {
        format!("unknown challenge {name:?}. Live ids: relearn, relearn-image, relearn-agent, bounty. relearn-mm is off and earns nothing.")
    })
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
    fn gateway_defaults_to_the_public_host() {
        let cli = Cli::try_parse_from(["ctx", "status"]).expect("parse");
        assert_eq!(cli.gateway, "https://network.cortex.foundation");
    }

    #[test]
    fn gateway_can_be_overridden_for_a_local_stack() {
        let cli = Cli::try_parse_from(["ctx", "--gateway", "http://127.0.0.1:8080", "weights"])
            .expect("parse");
        assert_eq!(cli.gateway, "http://127.0.0.1:8080");
    }

    #[test]
    fn submit_parses_repeatable_manifest_flags() {
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
            "--train-id",
            "2",
            "--train-dataset",
            "my-mix",
        ])
        .expect("parse");
        match cli.cmd {
            Cmd::Relearn {
                cmd: ChallengeCmd::Submit(args),
            } => {
                assert_eq!(args.train_ids, vec![1, 2]);
                assert_eq!(args.train_datasets, vec!["my-mix".to_owned()]);
            }
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn image_has_a_prompts_command_and_the_others_do_not() {
        assert!(Cli::try_parse_from(["ctx", "image", "prompts"]).is_ok());
        assert!(Cli::try_parse_from(["ctx", "relearn", "prompts"]).is_err());
    }

    /// Pairing is blocking, and a miner who has not accepted terms should be
    /// told so by the CLI rather than by a 403 from the gateway.
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
    }

    #[test]
    fn text_arg_prefers_inline_then_file() {
        assert_eq!(
            text_arg(Some("inline".into()), None, "--body").expect("inline"),
            "inline"
        );
        assert!(text_arg(None, None, "--body").is_err());
    }

    #[test]
    fn unknown_challenge_names_the_live_ids() {
        let err = resolve("relearn-mm").expect_err("off");
        assert!(err.contains("relearn-mm is off"), "{err}");
    }
}
