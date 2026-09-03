//! The four live challenges, and the read-only views over them.

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

/// Live challenges. `relearn-mm` is off: no trust-root row, no emission.
pub const LIVE: [Challenge; 4] = [
    Challenge {
        id: "relearn",
        label: "Relearn",
        command: "relearn",
        entry: "ctx relearn submit",
        emission_bps: 4000,
        work: "post-train Qwen/Qwen3.8-27B",
        guide: "docs/external-miner/relearn.md",
    },
    Challenge {
        id: "relearn-image",
        label: "Relearn Image",
        command: "image",
        entry: "ctx image submit",
        emission_bps: 1500,
        work: "fine-tune nvidia/Cosmos3-Super-Text2Image, judged by Q-Judger",
        guide: "docs/external-miner/relearn-image.md",
    },
    Challenge {
        id: "relearn-agent",
        label: "Relearn Agent",
        command: "agent",
        entry: "ctx agent submit",
        emission_bps: 1500,
        work: "post-train the same checkpoint into a tool-using agent",
        guide: "docs/external-miner/relearn-agent.md",
    },
    Challenge {
        id: "bounty",
        label: "Bounty",
        command: "bounty",
        entry: "ctx bounty pair, then ctx bounty report",
        emission_bps: 3000,
        work: "file real bug reports against the subnet",
        guide: "docs/external-miner/bounty.md",
    },
];

/// Resolve a challenge by id or by its `ctx` subcommand name.
pub fn find(name: &str) -> Option<&'static Challenge> {
    LIVE.iter()
        .find(|c| c.id == name || c.command == name)
        .or_else(|| match name {
            "relearn-t2i" | "t2i" => find("relearn-image"),
            _ => None,
        })
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
    println!("relearn-mm (encoder-attach multimodal) is off: no trust-root row, no");
    println!("emission. Submitting to it earns nothing.");
    println!();
    println!("The three Relearn challenges score on a private holdout, so winning the");
    println!("published split proves nothing. Bounty pays precision times severity.");
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
            if c.id != want {
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
                    println!("{:<14} unreachable: {e}", c.id);
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
    let can_score = body.get("can_score").and_then(Value::as_bool);
    let scoring = match can_score {
        Some(true) => "can_score: yes",
        Some(false) => "can_score: NO (submits answer 503)",
        None => "can_score: not reported",
    };
    println!("{:<14} {:>5} bps  {scoring}", c.id, c.emission_bps);
    for field in [
        "eval_backend",
        "judge_backend",
        "scoring_backend",
        "champion_hotkey",
        "base_weights",
        "live_harvest_wired",
        "champion_baseline_recorded",
    ] {
        if let Some(v) = body.get(field) {
            println!("               {field}: {}", compact(v));
        }
    }
    println!();
}

async fn print_weights_line(client: &Client) {
    match client.get("/v1/weights/latest").await {
        Ok(reply) if reply.ok() => {
            let sealed = reply
                .body
                .get("sealed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let epoch = reply.body.get("epoch").map_or_else(
                || "none".to_owned(),
                |v| compact(v).trim_matches('"').to_owned(),
            );
            if sealed {
                println!("weights: sealed epoch {epoch}");
            } else {
                println!(
                    "weights: NOT sealed — burn vector (uid 0 = 100%). Nothing is being paid."
                );
            }
        }
        Ok(reply) => println!("weights: HTTP {} {}", reply.status, reply.message()),
        Err(e) => println!("weights: unreachable: {e}"),
    }
}

/// Print the sealed weights vector summary.
pub async fn print_weights(client: &Client, json: bool) -> Result<(), String> {
    let reply = client.get("/v1/weights/latest").await?;
    if json {
        println!("{}", reply.body);
        return Ok(());
    }
    if !reply.ok() {
        return Err(format!("HTTP {}: {}", reply.status, reply.message()));
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
        println!("stale or invented one.");
    }
    Ok(())
}

/// Render a JSON value on one line, unquoting plain strings.
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
    fn four_live_challenges_sum_to_ten_thousand_bps() {
        let total: u32 = LIVE.iter().map(|c| c.emission_bps).sum();
        assert_eq!(total, 10_000);
        assert_eq!(LIVE.len(), 4);
    }

    #[test]
    fn ids_are_the_normative_ones() {
        let ids: Vec<&str> = LIVE.iter().map(|c| c.id).collect();
        assert_eq!(ids, ["relearn", "relearn-image", "relearn-agent", "bounty"]);
        assert!(!ids.contains(&"relearn-mm"));
    }

    #[test]
    fn lookup_accepts_ids_commands_and_the_internal_t2i_spelling() {
        assert_eq!(find("relearn-image").map(|c| c.id), Some("relearn-image"));
        assert_eq!(find("image").map(|c| c.id), Some("relearn-image"));
        assert_eq!(find("t2i").map(|c| c.id), Some("relearn-image"));
        assert_eq!(find("agent").map(|c| c.id), Some("relearn-agent"));
        assert!(find("relearn-mm").is_none());
    }

    #[test]
    fn compact_unquotes_strings() {
        assert_eq!(compact(&Value::String("lium".into())), "lium");
        assert_eq!(compact(&serde_json::json!(true)), "true");
    }
}
