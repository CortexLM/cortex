//! `cortex-bounty` — superseded pairing CLI, kept for operator scripts.
//!
//! Miners use `ctx bounty pair` (see `bins/ctx`), which signs the same
//! challenge and then binds the hotkey through the public gateway instead of
//! leaving the miner to carry a Chat inject command around.
//!
//! Never asks for a mnemonic in Chat. Sign locally with a wallet file or
//! `--secret-file`, or print the challenge and attach `--signature` after an
//! offline sr25519 sign.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use bounty_challenge_task::{
    chat_command_display, hotkey_ss58, pairing_code, parse_hotkey, parse_signature,
    public_from_mini_secret, sign_pair_challenge, validate_account_id, verify_pair_signature,
    PairChallenge, CHAT_COMMAND_PLACEHOLDER, DEFAULT_PAIR_TTL_SECS, TERMS_TEXT,
};
use clap::{Parser, Subcommand};
use keystore::{load_hotkey, mini_secret_from_key_file, BittensorWallet};

/// Miner CLI for Bounty Challenge pairing.
#[derive(Debug, Parser)]
#[command(
    name = "cortex-bounty",
    about = "Pair a Bittensor hotkey to a Cortex Chat account for Bounty Challenge"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Build a pairing challenge, sign it, print the Chat inject + pairing code.
    Pair {
        /// Miner hotkey (SS58 or 64-hex). Used to select among several linked keys.
        #[arg(long)]
        hotkey: String,
        /// Cortex Chat account id (dedicated mining account, not a private personal one).
        #[arg(long)]
        account_id: String,
        /// Optional 32-byte mini-secret file. Never a mnemonic. Never paste this in Chat.
        #[arg(long, env = "BOUNTY_HOTKEY_SK_FILE")]
        secret_file: Option<PathBuf>,
        /// Bittensor wallets directory (default `$BT_WALLETS_PATH` or `~/.bittensor/wallets`).
        #[arg(long)]
        wallet_dir: Option<PathBuf>,
        /// Wallet name under `--wallet-dir`.
        #[arg(long)]
        wallet_name: Option<String>,
        /// Hotkey file name under the wallet (default `default`).
        #[arg(long, default_value = "default")]
        wallet_hotkey: String,
        /// Hex signature from an offline sign of the challenge string.
        #[arg(long)]
        signature: Option<String>,
        /// Pairing expiry unix seconds (default now + 15 min).
        #[arg(long)]
        exp: Option<u64>,
        /// Override nonce (hex, 16..=64). Random when omitted.
        #[arg(long)]
        nonce: Option<String>,
    },
}

/// Shown once per invocation so operator scripts still work while miners move
/// to the CLI the docs point at.
const DEPRECATION: &str =
    "cortex-bounty is superseded by `ctx bounty pair`, which pairs through the public gateway. \
     See docs/external-miner/bounty.md.";

fn main() -> ExitCode {
    let cli = Cli::parse();
    eprintln!("{DEPRECATION}");
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cortex-bounty: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Pair {
            hotkey,
            account_id,
            secret_file,
            wallet_dir,
            wallet_name,
            wallet_hotkey,
            signature,
            exp,
            nonce,
        } => cmd_pair(
            &hotkey,
            &account_id,
            secret_file.as_deref(),
            wallet_dir.as_deref(),
            wallet_name.as_deref(),
            &wallet_hotkey,
            signature.as_deref(),
            exp,
            nonce.as_deref(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_pair(
    hotkey_s: &str,
    account_id: &str,
    secret_file: Option<&std::path::Path>,
    wallet_dir: Option<&std::path::Path>,
    wallet_name: Option<&str>,
    wallet_hotkey: &str,
    signature: Option<&str>,
    exp: Option<u64>,
    nonce: Option<&str>,
) -> Result<(), String> {
    validate_account_id(account_id).map_err(|e| e.to_string())?;
    let want = parse_hotkey(hotkey_s).map_err(|e| e.to_string())?;
    let now = unix_now();
    let exp = exp.unwrap_or_else(|| now.saturating_add(DEFAULT_PAIR_TTL_SECS));
    if exp <= now {
        return Err("exp must be in the future".into());
    }
    let nonce = match nonce {
        Some(n) => n.to_ascii_lowercase(),
        None => random_nonce()?,
    };
    let challenge = PairChallenge {
        account_id: account_id.to_owned(),
        nonce,
        exp,
    };
    let encoded = challenge.encode().map_err(|e| e.to_string())?;

    let sig = if let Some(hex_s) = signature {
        parse_signature(hex_s).map_err(|e| e.to_string())?
    } else if let Some(path) = secret_file {
        let sk = mini_secret_from_key_file(path).map_err(|e| e.to_string())?;
        let pk = public_from_mini_secret(&sk).map_err(|e| e.to_string())?;
        if pk != want {
            return Err(
                "secret-file public key does not match --hotkey; pick the matching hotkey".into(),
            );
        }
        sign_pair_challenge(&sk, &encoded).map_err(|e| e.to_string())?
    } else if let Some(name) = wallet_name {
        let dir = wallet_dir.map_or_else(keystore::default_wallets_dir, PathBuf::from);
        let wallet = BittensorWallet::new(name, wallet_hotkey);
        let kp = load_hotkey(&dir, wallet.wallet_name(), wallet.hotkey_name())
            .map_err(|e| e.to_string())?;
        if *kp.public_key() != want {
            return Err(format!(
                "wallet hotkey {} does not match --hotkey {}; switch with --wallet-hotkey or --hotkey",
                kp.ss58_address(),
                hotkey_ss58(&want)
            ));
        }
        sign_pair_challenge(kp.expose_mini_secret(), &encoded).map_err(|e| e.to_string())?
    } else {
        print_unsigned(&encoded, &hotkey_ss58(&want), account_id);
        return Ok(());
    };

    verify_pair_signature(&want, &encoded, &sig).map_err(|e| e.to_string())?;
    let ss58 = hotkey_ss58(&want);
    let code = pairing_code(&encoded, &hex::encode(sig), &ss58);
    print_signed(&encoded, &code, &ss58, account_id);
    Ok(())
}

fn print_unsigned(challenge: &str, ss58: &str, account_id: &str) {
    println!("Bounty Challenge pairing (unsigned)");
    println!();
    println!("Terms (blocking — you must accept in Chat):");
    println!("{TERMS_TEXT}");
    println!();
    println!("Use a dedicated mining Cortex Chat account, not a private personal account.");
    println!("Account: {account_id}");
    println!("Hotkey:  {ss58}");
    println!();
    println!("Challenge string (sign with this hotkey, sr25519 / substrate context):");
    println!("{challenge}");
    println!();
    println!("Then re-run:");
    println!(
        "  cortex-bounty pair --hotkey {ss58} --account-id {account_id} --signature <128-hex>"
    );
    println!();
    print_chat_and_switch(ss58);
}

fn print_signed(challenge: &str, code: &str, ss58: &str, account_id: &str) {
    println!("Bounty Challenge pairing");
    println!();
    println!("Terms (blocking — you must accept in Chat):");
    println!("{TERMS_TEXT}");
    println!();
    println!("Use a dedicated mining Cortex Chat account, not a private personal account.");
    println!("Account: {account_id}");
    println!("Hotkey:  {ss58}");
    println!("Challenge: {challenge}");
    println!();
    println!("1) Chat inject command (env BOUNTY_CHAT_COMMAND; placeholder if unset):");
    println!("   {}", chat_command_display());
    println!();
    println!("2) One-time pairing code (paste after the inject command in Cortex Chat):");
    println!("   {code}");
    println!();
    print_chat_and_switch(ss58);
}

fn print_chat_and_switch(ss58: &str) {
    println!("3) If several hotkeys are linked, pick/switch with:");
    println!("   cortex-bounty pair --hotkey {ss58} --account-id <id> …");
    println!("   cortex-bounty pair --hotkey <other-ss58> --account-id <id> --wallet-name <w> --wallet-hotkey <name>");
    println!();
    println!("Never paste a mnemonic into Chat. Never commit {CHAT_COMMAND_PLACEHOLDER}.");
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_nonce() -> Result<String, String> {
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut buf)
        })
        .map_err(|e| format!("urandom: {e}"))?;
    Ok(hex::encode(buf))
}
