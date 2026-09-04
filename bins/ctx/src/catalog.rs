//! The two live challenges, and the read-only views over them.

use serde_json::Value;

use crate::api::{challenge_path, Client};

/// One live challenge as miners see it.
#[derive(Debug)]
pub struct Challenge {
    /// Challenge id used in routes, leaves, and the trust root.
    pub id: &'static str,
    /// Human name.
    pub label: &'static str,
    /// `ctx` subcommand group for this challenge.
    pub command: &'static str,
    /// How a miner starts working on it.
    pub entry: &'static str,
    /// Default emission share in basis points.
    pub emission_bps: u32,
    /// What a miner actually improves.
    pub work: &'static str,
    /// Miner guide in this repo.
    pub guide: &'static str,
}

/// Live challenges. Relearn*, Design, and Prism are off: no trust-root row.
pub const LIVE: [Challenge; 2] = [
    Challenge {
        id: "bounty",
        label: "Bounty",
        command: "bounty",
        entry: "ctx bounty pair, then ctx bounty report",
        emission_bps: 5000,
        work: "file real bug reports against the subnet",
        guide: "docs/external-miner/bounty.md",
    },
    Challenge {
        id: "proof",
        label: "Proof",
        command: "proof",
        entry: "ctx proof submit",
        emission_bps: 5000,
        work: "reproduce an operator-published research topic against a sealed baseline",
        guide: "docs/external-miner/proof.md",
    },
];

/// Resolve a challenge by id or by its `ctx` subcommand name.
#[must_use]
pub fn find(name: &str) -> Option<&'static Challenge> {
    LIVE.iter().find(|c| c.id == name || c.command == name)
}

/// Print the live challenge table.
pub fn print_challenges() {
    println!("Live Cortex challenges (emission in basis points of the subnet):");
    println!();
    for c in &LIVE {
        println!("  {} — {} ({} bps)", c.id, c.label, c.emission_bps);
        println!("      work:  {}", c.work);
        println!("      start: {}", c.entry);
        println!("      guide: {}", c.guide);
        println!();
    }
    println!("relearn, relearn-image, relearn-agent, relearn-mm, design, and prism");
    println!("are off: no trust-root row, no emission. Submitting to them earns nothing.");
    println!();
    println!("Bounty pays precision times severity. Proof pays the mean of per-topic");
    println!("lattices over currently open ids; an empty eval digest cannot score.");
    println!("Run 'ctx status' to see whether each host can score right now.");
}

/// Fetch `/v1/status` for one challenge.
pub async fn fetch_status(client: &Client, challenge_id: &str) -> Result<Value, String> {
    let reply = client
        .get(&challenge_path(challenge_id, "/v1/status"))
        .await?;
    if reply.ok() {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}: {}", reply.status, reply.message()))
    }
}

/// Print the status of every live challenge plus the sealed weights vector.
pub async fn print_status(client: &Client, only: Option<&str>, json: bool) -> Result<(), String> {
    let mut collected = serde_json::Map::new();
    if only.is_none() && !json {
        println!("gateway: {}", client.gateway());
        print_weights_line(client).await;
        println!();
    }
    for c in &LIVE {
        if let Some(want) = only {
            if c.id != want && c.command != want {
                continue;
            }
        }
        match fetch_status(client, c.id).await {
            Ok(body) => {
                if json {
                    collected.insert(c.id.to_owned(), body);
                } else {
                    print_challenge_status(c, &body);
                }
            }
            Err(e) => {
                if json {
                    collected.insert(c.id.to_owned(), serde_json::json!({"error": e}));
                } else {
                    println!("{:<8} unreachable: {e}", c.id);
                }
            }
        }
    }
    if json {
        println!("{}", Value::Object(collected));
    }
    Ok(())
}

fn print_challenge_status(c: &Challenge, body: &Value) {
    let scoring = match body.get("can_score").and_then(Value::as_bool) {
        Some(true) => "can_score: yes",
        Some(false) => "can_score: NO (submits answer 503)",
        None => "can_score: not reported",
    };
    println!("{:<8} {} — {scoring}", c.id, c.label);
    for field in [
        "eval_backend",
        "scoring_backend",
        "force_sim",
        "live_harvest_wired",
        "baseline_sealed",
        "eval_image_digest",
    ] {
        if let Some(v) = body.get(field) {
            println!("         {field}: {}", compact(v));
        }
    }
}

async fn print_weights_line(client: &Client) {
    match client.get("/v1/weights/latest").await {
        Ok(reply) if reply.ok() => {
            let sealed = reply.body.get("sealed").and_then(Value::as_bool);
            match sealed {
                Some(true) => {
                    let epoch = compact(reply.body.get("epoch").unwrap_or(&Value::Null));
                    println!("weights: sealed epoch={epoch}");
                }
                Some(false) => {
                    println!("weights: unsealed burn vector (uid 0 = 100%; not a submit path)");
                }
                None => println!("weights: {}", compact(&reply.body)),
            }
        }
        Ok(reply) => println!("weights: HTTP {}", reply.status),
        Err(e) => println!("weights: {e}"),
    }
}

/// Print `GET /v1/weights/latest`.
pub async fn print_weights(client: &Client, json: bool) -> Result<(), String> {
    let reply = client.get("/v1/weights/latest").await?;
    if json {
        println!("{}", reply.body);
    }
    if !reply.ok() {
        return Err(format!("HTTP {}: {}", reply.status, reply.message()));
    }
    if json {
        return Ok(());
    }
    println!("gateway: {}", client.gateway());
    for field in [
        "sealed",
        "epoch",
        "revision",
        "netuid",
        "merkle_root",
        "computed_at",
        "metagraph_block",
    ] {
        if let Some(v) = reply.body.get(field) {
            println!("{field}: {}", compact(v));
        }
    }
    if reply.body.get("sealed").and_then(Value::as_bool) != Some(true) {
        println!();
        println!("Not sealed is the fail-closed answer, not an outage: with no sealed");
        println!("bundle the gateway serves a burn vector (uid 0 = 100%) instead of a");
        println!("stale or invented one. Validators do not submit that vector.");
    }
    Ok(())
}

/// Render a JSON value on one line, unquoting plain strings.
#[must_use]
pub fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_live_challenges_sum_to_ten_thousand_bps() {
        let total: u32 = LIVE.iter().map(|c| c.emission_bps).sum();
        assert_eq!(total, 10_000);
        assert_eq!(LIVE.len(), 2);
    }

    #[test]
    fn ids_are_the_normative_ones() {
        let ids: Vec<&str> = LIVE.iter().map(|c| c.id).collect();
        assert_eq!(ids, ["bounty", "proof"]);
        for off in [
            "relearn",
            "relearn-image",
            "relearn-agent",
            "relearn-mm",
            "design",
            "prism",
        ] {
            assert!(find(off).is_none(), "{off} must not be live");
        }
    }

    #[test]
    fn lookup_accepts_ids_and_commands() {
        assert_eq!(find("bounty").map(|c| c.id), Some("bounty"));
        assert_eq!(find("proof").map(|c| c.id), Some("proof"));
    }

    #[test]
    fn compact_unquotes_strings() {
        assert_eq!(compact(&Value::String("lium".into())), "lium");
        assert_eq!(compact(&serde_json::json!(true)), "true");
    }
}
