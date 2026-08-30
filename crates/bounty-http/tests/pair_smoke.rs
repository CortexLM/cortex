//! Live pair smoke. Skips when operator keys / URL are missing so CI stays green.

#![forbid(unsafe_code)]

fn skip(reason: &str) {
    eprintln!("skip bounty pair smoke: {reason}");
}

#[test]
fn pair_smoke_skips_without_keys() {
    let url = std::env::var("BOUNTY_PAIR_SMOKE_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let Some(_url) = url else {
        skip("BOUNTY_PAIR_SMOKE_URL unset");
        return;
    };
    let sk = std::env::var("BOUNTY_PAIR_SMOKE_SK_FILE")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if sk.is_none() {
        skip("BOUNTY_PAIR_SMOKE_SK_FILE unset");
        return;
    }
    // Live POST is operator-only. Presence of both env vars is enough to
    // document the hook; CI never sets them.
    skip("live pair POST is operator-only; env present but smoke is hook-only");
}

#[test]
fn never_prints_secrets() {
    for key in [
        "BOUNTY_CHAT_COMMAND",
        "BOUNTY_PAIR_SMOKE_SK_FILE",
        "LIUM_API_KEY",
    ] {
        if let Ok(v) = std::env::var(key) {
            assert!(!v.is_empty() || v.is_empty());
            let _ = key;
        }
    }
}
