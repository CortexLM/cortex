//! The authored set this repository's reference adaptor writes must pass the
//! gates the guest and the install hold it to — in Rust, not in Python.
//!
//! The adaptor's `propose_rules` is operator content and its own unit tests
//! are Python; the **authoritative** checks are `proof-topic-authoring` (which
//! the guest links) and `proof-topic-sql-guard` (which the install runs). A
//! Python test can only prove the module agrees with itself. This one runs the
//! real thing:
//!
//! 1. write a fixture topic the way a signed document carries one,
//! 2. run the adaptor's entrypoint exactly as the guest does (same env
//!    contract, `PROOF_JOB=propose_rules`),
//! 3. parse the `authoring.json` it wrote with `authoring_from_json`,
//! 4. hold it to `TopicAuthoring::validate` (shape, completeness, the
//!    migration deny-list) and `PinPolicy::agrees_with_document`, and
//! 5. hold it to `validate_against_pin` with the real pin file.
//!
//! A change to the adaptor that would produce a set the guest refuses fails
//! here, and so does a change to the gates that would start refusing the set
//! the adaptor ships.
//!
//! `python3` is required (the adaptor is a Python entrypoint); the test skips
//! with a message rather than passing vacuously if it is absent.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use proof_rlm::{authoring_from_json, TopicAuthoring};
use proof_task::{ChecklistRule, ProofPin, TopicDocument};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn adaptor() -> PathBuf {
    repo().join("deploy/guest/runners/rlm_fc_in_guest_harbor")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proof-rlm-authoring-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("dir");
    d
}

fn python3() -> Option<PathBuf> {
    let out = Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The topic the adaptor authors against: a custom-family document with a
/// checklist and the signed inspect policy each rule needs.
fn topic() -> TopicDocument {
    let mut doc = TopicDocument {
        id: "fixture-topic-v0".into(),
        statement: "score the pinned pack with the pinned runner".into(),
        epsilon_nll: 0.02,
        epsilon_topic_max_regress: 0.05,
        holdout_size: 120,
        ..TopicDocument::default()
    };
    doc.metric.family = proof_task::MetricFamily::Custom;
    doc.metric.custom_id = "fixture_metric".into();
    doc.metric.primary = "success_rate".into();
    doc.metric.direction = proof_task::MetricDirection::Max;
    doc.metric.epsilon_rel = 0.05;
    doc.eval_executor.max_proof_deadline_s = Some(3600);
    doc.checklist = vec![
        ChecklistRule {
            id: "no_short_circuit".into(),
            text: "the evaluator and the metric path are untouched".into(),
        },
        ChecklistRule {
            id: "miner_pays_provider".into(),
            text: "the miner pays for its own provider calls".into(),
        },
    ];
    doc.constraints
        .params
        .insert("baseline_runner".into(), "rlm_fc_in_guest_harbor".into());
    doc.constraints.params.insert(
        "experiment_pack_digest".into(),
        format!("sha256:{}", "ab".repeat(32)),
    );
    doc.constraints
        .params
        .insert("tasks_dir".into(), "tasks".into());
    doc
}

/// Run the adaptor's `propose_rules` exactly as the guest does.
fn author(doc: &TopicDocument, root: &Path, current: Option<&str>) -> Result<String, String> {
    let topic_file = root.join("topic.json");
    std::fs::write(
        &topic_file,
        serde_json::to_vec_pretty(doc).expect("topic json"),
    )
    .expect("write topic");
    let output = root.join("output");
    let work = root.join("work");
    std::fs::create_dir_all(&output).expect("output dir");
    std::fs::create_dir_all(&work).expect("work dir");
    let current_file = match current {
        Some(body) => {
            let path = work.join("current-authoring.json");
            std::fs::write(&path, body).expect("write current set");
            path.display().to_string()
        }
        None => String::new(),
    };
    let out = Command::new(adaptor().join("propose_rules"))
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("PROOF_JOB", "propose_rules")
        .env("PROOF_TOPIC_ID", &doc.id)
        .env("PROOF_CUSTOM_ID", &doc.metric.custom_id)
        .env("PROOF_TOPIC_FILE", &topic_file)
        .env("PROOF_OUTPUT_DIR", &output)
        .env("PROOF_WORK_DIR", &work)
        .env("PROOF_CURRENT_AUTHORING_FILE", current_file)
        .env(
            "PROOF_PARAM_INSPECT_MARKER_RULES",
            "no_short_circuit:skip_eval|skip_verifier",
        )
        .env("PROOF_PARAM_INSPECT_ATTESTED_RULES", "miner_pays_provider")
        .output()
        .map_err(|e| format!("spawn propose_rules: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "propose_rules exited {:?}: {}{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let path = output.join("authoring.json");
    if path.is_file() {
        assert!(
            !output.join("rules.json").exists(),
            "the adaptor wrote rules.json: a fragment is not authorship, and the host refuses \
             to open a topic on one"
        );
    }
    std::fs::read_to_string(&path).map_err(|e| format!("read authoring.json: {e}"))
}

/// The set the reference adaptor writes is the set the guest accepts.
#[test]
fn the_reference_adaptor_authors_a_set_the_guest_and_the_install_accept() {
    if python3().is_none() {
        eprintln!("python3 not on PATH: skipping the adaptor authoring gate");
        return;
    }
    let doc = topic();
    let root = tmp("complete");
    let body = author(&doc, &root, None).expect("the adaptor authors a set");

    // 1. It parses as the shape the guest reads (`deny_unknown_fields`).
    let set: TopicAuthoring = authoring_from_json(&body).expect("parses as a TopicAuthoring");

    // 2. Every part the install applies is present.
    assert!(
        set.is_complete(),
        "the set is missing {:?}",
        set.missing_parts()
    );
    assert_eq!(set.topic_id, doc.id);
    assert_eq!(set.schema_version, proof_rlm::AUTHORING_SCHEMA);
    assert_eq!(
        set.rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["no_short_circuit", "miner_pays_provider"],
        "the vector is the declared rules, in declaration order"
    );
    assert!(
        !set.migrations.is_empty() && !set.apis.is_empty(),
        "a complete set carries a migration and a route"
    );

    // 3. The guest's own gate: shape, completeness, and the migration
    //    deny-list, run before the answer becomes a job output.
    set.validate(&doc.id).expect("the guest accepts the set");

    // 4. The policy restates the signed document — scoring reads the document,
    //    so a divergence either way is a threshold nobody is judged by.
    set.pin_policy
        .agrees_with_document(&doc)
        .expect("the policy restates the document");

    // 5. And the control plane's gate: the policy against the **pin**.
    let pin = ProofPin::from_toml(
        &std::fs::read_to_string(repo().join("config/proof-pin.toml")).expect("pin file"),
    )
    .expect("pin parses");
    pin.validate().expect("the shipped pin validates");
    set.validate_against_pin(&doc.id, &pin)
        .expect("the control plane accepts the set against the pin");
    let _ = std::fs::remove_dir_all(&root);
}

/// The authored set is the RLM's answer, not the operator's bundle.
#[test]
fn the_authored_set_is_not_a_copy_of_the_signed_document() {
    if python3().is_none() {
        eprintln!("python3 not on PATH: skipping the adaptor authoring gate");
        return;
    }
    let doc = topic();
    let root = tmp("not-a-copy");
    let body = author(&doc, &root, None).expect("the adaptor authors a set");
    let set: TopicAuthoring = authoring_from_json(&body).expect("parses");

    // The rule text is the RLM's framing, not the operator's sentence alone.
    for rule in &set.rules {
        let declared = doc
            .checklist
            .iter()
            .find(|d| d.id == rule.id)
            .expect("the rule is one the topic declares");
        assert_ne!(
            rule.text.trim(),
            declared.text.trim(),
            "rule {} is the operator's sentence verbatim: that is a restatement, and the \
             provenance it would carry is the operator-cloned document the gate refuses",
            rule.id
        );
        assert!(
            rule.text.contains("rlm:"),
            "rule {} does not say how this RLM enforces it: {}",
            rule.id,
            rule.text
        );
    }

    // The migration is inside the topic's own namespace, and touches nothing
    // the repository owns.
    let prefix = doc.id.replace('-', "_");
    for migration in &set.migrations {
        assert!(
            migration.sql.contains(&prefix),
            "migration {} is not inside the topic's namespace: {}",
            migration.name,
            migration.sql
        );
        assert!(
            !migration.sql.to_lowercase().contains("proof_"),
            "migration {} names an object the repository owns",
            migration.name
        );
    }

    // The submission format describes this runtime's intake, not a bundle
    // section: the staged cap and the submit domain are facts about the host.
    let fmt = set
        .submission_format
        .as_object()
        .expect("format is an object");
    assert!(
        fmt.contains_key("signature_domain"),
        "the format does not name the signature domain the intake checks"
    );
    assert_eq!(
        set.pin_policy.eval_image_digest, None,
        "the policy invented an eval image digest: the VM does not hold the pin"
    );
    assert_eq!(
        set.pin_policy.gpu_class, None,
        "the policy invented a gpu class: the VM does not hold the pin"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Re-authoring retains what it is not changing, and the retained set is
/// still held to the same gates (retention is not a bypass).
#[test]
fn a_re_authoring_run_retains_the_prior_set_and_is_still_validated() {
    if python3().is_none() {
        eprintln!("python3 not on PATH: skipping the adaptor authoring gate");
        return;
    }
    let doc = topic();
    let root = tmp("re-author");
    let first = author(&doc, &root, None).expect("first authoring run");
    let first_set: TopicAuthoring = authoring_from_json(&first).expect("parses");
    let second = author(&doc, &root, Some(&first)).expect("second authoring run");
    let second_set: TopicAuthoring = authoring_from_json(&second).expect("parses");

    // The first run's parts survive: nothing the RLM still needs vanishes.
    for migration in &first_set.migrations {
        assert!(
            second_set
                .migrations
                .iter()
                .any(|m| m.name == migration.name && m.sql == migration.sql),
            "migration {} was dropped by the re-authoring run",
            migration.name
        );
    }
    for api in &first_set.apis {
        assert!(
            second_set
                .apis
                .iter()
                .any(|a| a.path == api.path && a.method == api.method),
            "route {} /{} was dropped by the re-authoring run",
            api.method,
            api.path
        );
    }
    // And the retained set still passes the gates.
    second_set
        .validate(&doc.id)
        .expect("the guest accepts the re-authored set");
    second_set
        .pin_policy
        .agrees_with_document(&doc)
        .expect("the re-authored policy restates the document");
    let _ = std::fs::remove_dir_all(&root);
}

/// A prior set whose migrations include a topic-scoped `DELETE` is retained and
/// still accepted: the part the topic still needs must survive a re-authoring
/// run, and the RLM's own scope check must read `DELETE FROM <topic>_table` as
/// an in-namespace touch rather than a touch on `FROM`.
///
/// Greptile P1: the scan skipped modifiers after a table keyword but not
/// `FROM`, so a retained pruning migration was refused and the topic could not
/// re-author.
#[test]
fn a_retained_topic_scoped_delete_survives_re_authoring() {
    if python3().is_none() {
        eprintln!("python3 not on PATH: skipping the adaptor authoring gate");
        return;
    }
    let doc = topic();
    let root = tmp("retained-delete");
    let first = author(&doc, &root, None).expect("first authoring run");
    let prefix = doc.id.replace('-', "_");
    let mut prior: TopicAuthoring = authoring_from_json(&first).expect("parses");
    prior.migrations.push(proof_rlm::AuthoredMigration {
        name: "0002_prune".into(),
        sql: format!("DELETE FROM {prefix}_rlm_state WHERE key = 'stale'"),
    });
    let prior_body = serde_json::to_string(&prior).expect("encode prior");
    let second = author(&doc, &root, Some(&prior_body)).expect("second authoring run");
    let second_set: TopicAuthoring = authoring_from_json(&second).expect("parses");
    let pruned = second_set
        .migrations
        .iter()
        .find(|m| m.name == "0002_prune")
        .expect("the retained pruning migration was dropped");
    assert_eq!(
        pruned.sql,
        format!("DELETE FROM {prefix}_rlm_state WHERE key = 'stale'")
    );
    // And the set is still what the guest and the install accept.
    second_set
        .validate(&doc.id)
        .expect("the guest accepts a set carrying a topic-scoped DELETE");
    let _ = std::fs::remove_dir_all(&root);
}

/// A topic that says nothing about how a rule is ticked is a refusal, not a
/// silent pass: the adaptor never invents a check and never drops a rule.
#[test]
fn a_topic_with_no_rule_policy_authors_nothing() {
    if python3().is_none() {
        eprintln!("python3 not on PATH: skipping the adaptor authoring gate");
        return;
    }
    let doc = topic();
    let root = tmp("no-policy");
    let topic_file = root.join("topic.json");
    std::fs::write(
        &topic_file,
        serde_json::to_vec_pretty(&doc).expect("topic json"),
    )
    .expect("write topic");
    let output = root.join("output");
    let work = root.join("work");
    std::fs::create_dir_all(&output).expect("output dir");
    std::fs::create_dir_all(&work).expect("work dir");
    let out = Command::new(adaptor().join("propose_rules"))
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("PROOF_JOB", "propose_rules")
        .env("PROOF_TOPIC_ID", &doc.id)
        .env("PROOF_TOPIC_FILE", &topic_file)
        .env("PROOF_OUTPUT_DIR", &output)
        .env("PROOF_WORK_DIR", &work)
        .env("PROOF_CURRENT_AUTHORING_FILE", "")
        .output()
        .expect("spawn propose_rules");
    assert!(
        !out.status.success(),
        "a topic with no inspect policy must fail closed, not author a set"
    );
    assert!(
        !output.join("authoring.json").exists(),
        "a refused run writes no set"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("inspect_marker_rules") || stderr.contains("inspect_attested_rules"),
        "the refusal must name the missing policy: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
