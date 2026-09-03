//! Fail if external miner docs drift from `bundle` `PROTOCOL_VERSION`, from the
//! gateway host `ctx` ships with, or from the live challenge product rules; if
//! relearn HTTP miner paths are missing; or if `docs/THREAT_MODEL.md` D19 claim
//! is not word-for-word vs plan pin.

use std::fs;
use std::path::Path;

/// Plan D19 claim body (after "verbatim in docs:"). Must match `THREAT_MODEL` section 1.
const D19_VERBATIM: &str = "base guarantees *no equivocation between validators* and *no undetected deviation by the gateway from the owner-signed challenge and measurement artifacts*. It does **not** guarantee (i) that a challenge's scores are honest, (ii) that the owner is honest — the owner signs the trust roots and runs the gateway, so a malicious owner can authorize a dishonest challenge or a backdoored measurement, (iii) completeness beyond what D24 provides, nor (iv) **chain-anchored, third-party-auditable non-equivocation** — per D5 the property is peer-consensus plus local evidence, verifiable by the participating validators and not by an outside observer after the fact.";

/// Marker comment required in external miner docs.
const BADGE_COMMENT_PREFIX: &str = "<!-- protocol_version:";

/// Content pins required in `docs/external-miner/README.md`.
const EXTERNAL_MINER_PINS: &[(&str, &str)] = &[
    ("relearn_challenge", "relearn"),
    ("relearn_image_challenge", "relearn-image"),
    ("relearn_agent_challenge", "relearn-agent"),
    ("relearn_mm_off", "relearn-mm"),
    ("bounty_challenge", "bounty"),
    ("http_submit", "HTTP"),
    ("lium_byok", "X-Lium-Api-Key"),
    ("lium_pay", "Miner pays Lium"),
    ("bundle_spec_link", "BUNDLE_SPEC.md"),
    ("base_model", "Qwen/Qwen3.8-27B"),
    ("teacher_nvfp4", "incoai/GLM-5.3-NVFP4"),
    ("teacher_model", "glm-5.3"),
    ("image_base_model", "nvidia/Cosmos3-Super-Text2Image"),
    ("image_judge", "Qwen/Qwen-Image-Bench"),
    ("image_flux_rejected", "Flux is rejected"),
    ("agent_trace_scoring", "replayed tool traces"),
    ("mm_encoder", "google/siglip2-so400m-patch14-384"),
    // A live challenge that never says what it will not pay for teaches
    // miners to find out by losing money.
    ("private_holdout", "private holdout"),
    ("off_score_gate", "off the number you are paid on"),
    ("fail_closed", "fails closed"),
    ("bounty_backend_consumer", "CortexLM/backend"),
    // A miner who cannot install the CLI cannot follow any of the guides.
    ("ctx_install_script", "scripts/install-ctx.sh"),
    ("ctx_help", "ctx --help"),
    ("ctx_status", "ctx status"),
    ("can_score", "can_score"),
];

/// Every miner page must name the real gateway host, not a placeholder.
///
/// The value is read from `bins/ctx`, so the docs and the binary's default
/// cannot drift apart.
const GATEWAY_CONST_FILE: &str = "bins/ctx/src/api.rs";

/// Const declaration the host is parsed out of.
const GATEWAY_CONST_PREFIX: &str = "pub const DEFAULT_GATEWAY: &str =";

/// Pages a miner reads. They must resolve every host and never hand a miner an
/// operator env var to set.
const MINER_PAGES: &[&str] = &[
    "README.md",
    "relearn.md",
    "relearn-image.md",
    "relearn-agent.md",
    "relearn-mm.md",
    "bounty.md",
    "troubleshoot.md",
];

/// Strings that turn a miner guide into an operator runbook. Bounty Chat
/// activation is shown by `ctx` after pairing; the scorer feed and the gateway
/// endpoint are host config. A miner who is told to export one of these has
/// been handed a secret they cannot have.
///
/// `validators.md` is exempt: a validator does set `BASE_GATEWAY_ENDPOINT` on
/// its own process.
const MINER_FORBIDDEN_ENV: &[&str] = &[
    "BOUNTY_CHAT_COMMAND",
    "BOUNTY_BACKEND_PUBLIC_URL",
    "BASE_GATEWAY_",
];

/// Per-page pins. A challenge product rule that is not in the miner's own guide
/// is not a rule they will follow.
const PAGE_PINS: &[(&str, &[&str])] = &[
    (
        "relearn-image.md",
        &[
            "nvidia/Cosmos3-Super-Text2Image",
            "OpenMDW 1.1",
            "Qwen/Qwen-Image-Bench",
            "Flux is rejected",
            "Q-Judger is the only judge",
            // The gates a miner has to design for, in their own guide.
            "Capability canary",
            "contamination_evidence_missing",
        ],
    ),
    (
        "relearn-agent.md",
        &[
            "Qwen/Qwen3.8-27B",
            "Trace replay",
            "Tool ablation",
            "Observation shuffle",
            "without using the image or",
            "contamination_evidence_missing",
        ],
    ),
    (
        "bounty.md",
        &[
            // Bounty pays on precision x severity, and the canary that can
            // zero a miner is not in the number they see.
            "severity",
            "triage noise",
            "informational",
            "can_score",
        ],
    ),
    (
        "relearn-mm.md",
        &[
            "google/siglip2-so400m-patch14-384",
            "Apache-2.0",
            "zero on this challenge",
            "shuffled",
        ],
    ),
];

/// An Image page that names Flux as an allowed base is a product bug, not a typo.
const T2I_FORBIDDEN_BASES: &[&str] = &["flux.1-dev", "flux.1-schnell", "flux.1-pro"];

/// Substrings that must not appear as live miner guidance (removed path).
const FORBIDDEN_LIVE_PATHS: &[&str] = &[
    "phala deploy",
    "Phala CVM miner",
    "install.sh",
    "compose-hash",
    "funding-phala",
];

/// Run external-docs + D19 gates.
///
/// # Errors
///
/// Returns a multi-line error when any pin fails.
pub fn run(workspace_root: &Path) -> Result<(), String> {
    let mut failures = Vec::new();

    let protocol_version = read_bundle_protocol_version(workspace_root)?;
    let gateway = read_ctx_default_gateway(workspace_root)?;
    check_external_miner_docs(workspace_root, protocol_version, &mut failures)?;
    check_gateway_host(workspace_root, &gateway, &mut failures)?;
    check_threat_model_d19(workspace_root, &mut failures)?;
    check_threat_model_supporting_pins(workspace_root, &mut failures)?;

    if failures.is_empty() {
        println!(
            "external-docs-check OK (protocol_version={protocol_version}, gateway={gateway}, relearn + relearn-image + relearn-agent + bounty HTTP, D19 verbatim match)"
        );
        Ok(())
    } else {
        Err(format!(
            "external-docs-check failed ({}):\n{}",
            failures.len(),
            failures
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

fn read_bundle_protocol_version(workspace_root: &Path) -> Result<u16, String> {
    let types_rs = workspace_root.join("crates/bundle/src/types.rs");
    let body =
        fs::read_to_string(&types_rs).map_err(|e| format!("read {}: {e}", types_rs.display()))?;
    // pub const PROTOCOL_VERSION: u16 = 1;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("pub const PROTOCOL_VERSION") {
            let Some(eq) = t.split('=').nth(1) else {
                continue;
            };
            let num = eq
                .trim()
                .trim_end_matches(';')
                .trim()
                .parse::<u16>()
                .map_err(|e| format!("parse PROTOCOL_VERSION from {t:?}: {e}"))?;
            return Ok(num);
        }
    }
    Err(format!(
        "PROTOCOL_VERSION const not found in {}",
        types_rs.display()
    ))
}

/// Every `.md` under a directory, recursively.
fn markdown_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).map_err(|e| format!("read_dir {}: {e}", next.display()))? {
            let path = entry.map_err(|e| format!("dirent: {e}"))?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Read `DEFAULT_GATEWAY` out of the `ctx` CLI so docs cannot name a different
/// host than the binary miners install.
fn read_ctx_default_gateway(workspace_root: &Path) -> Result<String, String> {
    let path = workspace_root.join(GATEWAY_CONST_FILE);
    let body = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(GATEWAY_CONST_PREFIX) {
            let host = rest.trim().trim_end_matches(';').trim().trim_matches('"');
            if host.starts_with("https://") && !host.ends_with('/') {
                return Ok(host.to_owned());
            }
            return Err(format!(
                "{GATEWAY_CONST_FILE}: DEFAULT_GATEWAY must be an https URL without a trailing slash, got {host:?}"
            ));
        }
    }
    Err(format!(
        "{GATEWAY_CONST_FILE} has no `{GATEWAY_CONST_PREFIX} \"…\"` line"
    ))
}

/// Miner docs must resolve every host, and must not hand a miner operator env.
///
/// A guide with a `<gateway>` placeholder is a guide the miner cannot run: they
/// either guess a host or give up, and both look like the subnet being closed.
fn check_gateway_host(
    workspace_root: &Path,
    gateway: &str,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let dir = workspace_root.join("docs/external-miner");
    if !dir.is_dir() {
        return Ok(());
    }

    // Recursive: the seed copies of the public miner repos live in
    // subdirectories, and a placeholder host there is just as unusable.
    for path in markdown_files(&dir)? {
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let body = fs::read_to_string(&path).map_err(|e| format!("read {rel}: {e}"))?;
        for placeholder in ["<gateway>", "<GATEWAY>", "<host>"] {
            if body.contains(placeholder) {
                failures.push(format!(
                    "{rel} still has the {placeholder:?} placeholder; write {gateway} instead"
                ));
            }
        }
    }

    for page in MINER_PAGES {
        let path = dir.join(page);
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        if !body.contains(gateway) {
            failures.push(format!(
                "docs/external-miner/{page} never names the gateway {gateway}"
            ));
        }
        for banned in MINER_FORBIDDEN_ENV {
            if body.contains(banned) {
                failures.push(format!(
                    "docs/external-miner/{page} tells a miner about operator env {banned:?}; \
                     use a ctx command or a concrete URL"
                ));
            }
        }
    }

    // The top-level README is the first thing a miner reads.
    let root_readme = workspace_root.join("README.md");
    let body = fs::read_to_string(&root_readme).map_err(|e| format!("read README.md: {e}"))?;
    for needle in [gateway, "scripts/install-ctx.sh", "ctx challenges"] {
        if !body.contains(needle) {
            failures.push(format!("README.md missing miner pin {needle:?}"));
        }
    }
    Ok(())
}

fn check_external_miner_docs(
    workspace_root: &Path,
    expected: u16,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let dir = workspace_root.join("docs/external-miner");
    if !dir.is_dir() {
        failures.push(format!("missing directory {}", dir.display()));
        return Ok(());
    }

    let readme = dir.join("README.md");
    let readme_body =
        fs::read_to_string(&readme).map_err(|e| format!("read {}: {e}", readme.display()))?;
    match extract_badge_version(&readme_body) {
        Ok(v) if v == expected => {}
        Ok(v) => failures.push(format!(
            "docs/external-miner/README.md protocol_version badge={v} != bundle PROTOCOL_VERSION={expected}"
        )),
        Err(e) => failures.push(format!("docs/external-miner/README.md: {e}")),
    }

    for (name, needle) in EXTERNAL_MINER_PINS {
        if !readme_body.contains(needle) {
            failures.push(format!(
                "docs/external-miner/README.md missing pin {name}: {needle:?}"
            ));
        }
    }

    // Required pages for live HTTP submit.
    for required in [
        "relearn.md",
        "relearn-image.md",
        "relearn-agent.md",
        "relearn-mm.md",
        "bounty.md",
        "troubleshoot.md",
    ] {
        let path = dir.join(required);
        if !path.is_file() {
            failures.push(format!(
                "docs/external-miner/{required} missing (HTTP submit guide required)"
            ));
        }
    }

    for (page, pins) in PAGE_PINS {
        let path = dir.join(page);
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for needle in *pins {
            if !body.contains(needle) {
                failures.push(format!("docs/external-miner/{page} missing pin {needle:?}"));
            }
        }
    }

    // The Image guide must reject Flux, never offer it as a base.
    let t2i = dir.join("relearn-image.md");
    if let Ok(body) = fs::read_to_string(&t2i) {
        let lower = body.to_ascii_lowercase();
        for banned in T2I_FORBIDDEN_BASES {
            let mentioned_as_allowed = lower
                .split('\n')
                .any(|l| l.contains(banned) && !l.contains("reject") && !l.contains("refus"));
            if mentioned_as_allowed {
                failures.push(format!(
                    "docs/external-miner/relearn-image.md mentions {banned:?} outside a rejection"
                ));
            }
        }
    }

    // Every markdown file under external-miner must declare the same badge comment.
    let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for ent in entries {
        let ent = ent.map_err(|e| format!("dirent: {e}"))?;
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let body =
            fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        match extract_badge_version(&body) {
            Ok(v) if v == expected => {}
            Ok(v) => failures.push(format!(
                "{} badge protocol_version={v} != {expected}",
                path.strip_prefix(workspace_root).unwrap_or(&path).display()
            )),
            Err(e) => failures.push(format!(
                "{}: {e}",
                path.strip_prefix(workspace_root).unwrap_or(&path).display()
            )),
        }

        let lower = body.to_ascii_lowercase();
        for banned in FORBIDDEN_LIVE_PATHS {
            if lower.contains(&banned.to_ascii_lowercase()) {
                failures.push(format!(
                    "{} contains removed miner-path string {banned:?} (use relearn HTTP only)",
                    path.strip_prefix(workspace_root).unwrap_or(&path).display()
                ));
            }
        }
    }

    Ok(())
}

fn extract_badge_version(body: &str) -> Result<u16, String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(BADGE_COMMENT_PREFIX) {
            let num = rest
                .trim()
                .trim_end_matches("-->")
                .trim()
                .parse::<u16>()
                .map_err(|e| format!("bad protocol_version comment {t:?}: {e}"))?;
            return Ok(num);
        }
    }
    // Also accept bold badge line: **Bundle `protocol_version`:** `1`
    for line in body.lines() {
        if line.contains("protocol_version") && line.contains('`') {
            // find last `N` numeric
            let mut last = None;
            let mut cur = String::new();
            let mut in_tick = false;
            for ch in line.chars() {
                if ch == '`' {
                    if in_tick {
                        if let Ok(v) = cur.parse::<u16>() {
                            last = Some(v);
                        }
                        cur.clear();
                        in_tick = false;
                    } else {
                        in_tick = true;
                        cur.clear();
                    }
                } else if in_tick {
                    cur.push(ch);
                }
            }
            if let Some(v) = last {
                return Ok(v);
            }
        }
    }
    Err(format!(
        "missing `{BADGE_COMMENT_PREFIX} N -->` badge (and no parseable protocol_version backticks)"
    ))
}

fn check_threat_model_d19(workspace_root: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let path = workspace_root.join("docs/THREAT_MODEL.md");
    let body = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    // Prefer fenced section after the D19 heading.
    let Some(idx) = body.find("## 1. D19") else {
        failures.push("docs/THREAT_MODEL.md missing heading `## 1. D19`".into());
        return Ok(());
    };
    let rest = &body[idx..];
    let Some(after_blank) = rest.split("\n\n").nth(2) else {
        failures.push("docs/THREAT_MODEL.md: could not locate D19 claim paragraph".into());
        return Ok(());
    };
    // Paragraph until next blank line or heading
    let para = after_blank
        .lines()
        .take_while(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let para = para.trim();

    // Also try: find the claim by unique prefix if structure drifts.
    let matched = para == D19_VERBATIM || body.contains(D19_VERBATIM);

    if !matched {
        failures.push(format!(
            "docs/THREAT_MODEL.md D19 claim is not word-for-word vs plan D19.\n  expected (first 120 chars): {:?}\n  found paragraph (first 120): {:?}",
            D19_VERBATIM.chars().take(120).collect::<String>(),
            para.chars().take(120).collect::<String>()
        ));
    }
    Ok(())
}

fn check_threat_model_supporting_pins(
    workspace_root: &Path,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let path = workspace_root.join("docs/THREAT_MODEL.md");
    let body = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let pins = [
        ("D11", "do **not** claim env **values** are verified"),
        ("D5_payload", "WeightsTlockPayload"),
        (
            "D5_negation",
            "merkle root is NOT committed in the on-chain weight payload",
        ),
        (
            "R12",
            "The owner is the trust root AND the gateway operator",
        ),
        ("peer_evidence", "peer-consensus plus local evidence"),
    ];
    for (name, needle) in pins {
        if !body.contains(needle) {
            failures.push(format!(
                "docs/THREAT_MODEL.md missing pin {name}: {needle:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_badge_from_comment() {
        let s = "<!-- protocol_version: 1 -->\nhello\n";
        assert_eq!(extract_badge_version(s).unwrap(), 1);
    }

    #[test]
    fn d19_constant_nonempty() {
        assert!(D19_VERBATIM.contains("no equivocation between validators"));
        assert!(D19_VERBATIM.contains("chain-anchored, third-party-auditable non-equivocation"));
    }

    #[test]
    fn external_pins_cover_relearn() {
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, _)| *n == "relearn_challenge"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, v)| *n == "base_model" && *v == "Qwen/Qwen3.8-27B"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, v)| *n == "teacher_nvfp4" && *v == "incoai/GLM-5.3-NVFP4"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, v)| *n == "teacher_model" && *v == "glm-5.3"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, v)| *n == "lium_pay" && *v == "Miner pays Lium"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, _)| *n == "bounty_challenge"));
        assert!(EXTERNAL_MINER_PINS
            .iter()
            .any(|(n, v)| *n == "bounty_backend_consumer" && *v == "CortexLM/backend"));
    }

    /// The index used to require the operator env names as pins, which is how
    /// miner docs ended up instructing miners to export a Chat token.
    #[test]
    fn the_index_does_not_pin_operator_env_names() {
        for (_, value) in EXTERNAL_MINER_PINS {
            assert!(
                !MINER_FORBIDDEN_ENV.iter().any(|b| value.contains(b)),
                "pin {value:?} names operator env"
            );
        }
    }

    #[test]
    fn miner_pages_must_not_hand_out_operator_env() {
        for banned in ["BOUNTY_CHAT_COMMAND", "BOUNTY_BACKEND_PUBLIC_URL"] {
            assert!(MINER_FORBIDDEN_ENV.contains(&banned), "missing {banned}");
        }
        // Validators legitimately set their own gateway endpoint.
        assert!(!MINER_PAGES.contains(&"validators.md"));
    }

    #[test]
    fn the_ctx_default_gateway_is_the_documented_host() {
        let root = workspace_root();
        let gateway = read_ctx_default_gateway(&root).expect("ctx gateway const");
        assert_eq!(gateway, "https://network.cortex.foundation");
    }

    /// The whole gate, against this repo. Docs drift is a test failure, not
    /// something that waits for the xtask lane.
    #[test]
    fn gate_passes_on_this_workspace() {
        super::run(&workspace_root()).expect("external-docs-check should pass");
    }

    fn workspace_root() -> std::path::PathBuf {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent()
            .map_or_else(std::path::PathBuf::new, Path::to_path_buf)
    }

    #[test]
    fn external_pins_cover_the_four_live_ids() {
        for (name, value) in [
            ("relearn_challenge", "relearn"),
            ("relearn_image_challenge", "relearn-image"),
            ("relearn_agent_challenge", "relearn-agent"),
            ("bounty_challenge", "bounty"),
            ("image_base_model", "nvidia/Cosmos3-Super-Text2Image"),
            ("image_judge", "Qwen/Qwen-Image-Bench"),
            ("image_flux_rejected", "Flux is rejected"),
            ("agent_trace_scoring", "replayed tool traces"),
            ("mm_encoder", "google/siglip2-so400m-patch14-384"),
        ] {
            assert!(
                EXTERNAL_MINER_PINS
                    .iter()
                    .any(|(n, v)| *n == name && *v == value),
                "missing pin {name}"
            );
        }
    }

    /// A miner who cannot see what the gates are will find them by losing
    /// money, so the index has to say them out loud.
    #[test]
    fn external_pins_name_the_incentive_rules() {
        for name in ["private_holdout", "off_score_gate", "fail_closed"] {
            assert!(
                EXTERNAL_MINER_PINS.iter().any(|(n, _)| *n == name),
                "missing pin {name}"
            );
        }
    }

    #[test]
    fn page_pins_hold_the_product_rules() {
        let t2i = PAGE_PINS
            .iter()
            .find(|(p, _)| *p == "relearn-image.md")
            .map(|(_, pins)| *pins)
            .unwrap_or_default();
        assert!(t2i.contains(&"Flux is rejected"));
        assert!(t2i.contains(&"Q-Judger is the only judge"));
        let mm = PAGE_PINS
            .iter()
            .find(|(p, _)| *p == "relearn-mm.md")
            .map(|(_, pins)| *pins)
            .unwrap_or_default();
        assert!(mm.contains(&"zero on this challenge"));
        assert!(mm.contains(&"shuffled"));

        // The Agent page has to state the arms that separate it from a prompt
        // benchmark, or the challenge reads as "relearn with extra words".
        let agent = PAGE_PINS
            .iter()
            .find(|(p, _)| *p == "relearn-agent.md")
            .map(|(_, pins)| *pins)
            .unwrap_or_default();
        for arm in ["Trace replay", "Tool ablation", "Observation shuffle"] {
            assert!(agent.contains(&arm), "agent page must pin {arm:?}");
        }
    }

    #[test]
    fn t2i_forbidden_bases_name_the_flux_variants() {
        assert!(T2I_FORBIDDEN_BASES.contains(&"flux.1-dev"));
        assert!(T2I_FORBIDDEN_BASES.contains(&"flux.1-schnell"));
        assert!(T2I_FORBIDDEN_BASES.contains(&"flux.1-pro"));
    }
}
