//! `FirecrackerOrchestrator` against an in-process `proof-vm-orchestrator`
//! agent over a **fake** hypervisor. No Firecracker, no VM, no network beyond
//! loopback — exactly what CI runs.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use proof_rlm::fixtures::{pinned_template, request, rules, token_for, FakeOrchestrator};
use proof_rlm::{
    CustomRunner, RetainPolicy, RunnerError, TopicVmOrchestrator, TopicVmSpec, VmBackedRunner,
    VmError, VmHandle, VmJob, VmJobOutput, VmTemplate, RLM_VM_IMAGE_DIGEST_ENV,
    VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
use proof_vm_agent::fixtures::{token_file, FakeAgent, FakeHypervisor};
use proof_vm_fc::{
    FcConfig, FcConfigError, FirecrackerOrchestrator, DEFAULT_RLM_MEM_MIB, DEFAULT_RLM_VCPUS,
    RLM_VM_MEM_MIB_ENV, RLM_VM_VCPUS_ENV,
};
use url::Url;

const TOKEN: &str = "fc-client-test-token-not-a-real-secret";

static ENV: Mutex<()> = Mutex::new(());

fn client(agent: &FakeAgent, token: &Path, digest: &str) -> FirecrackerOrchestrator {
    let mut cfg = FcConfig::new(Url::parse(&agent.url()).expect("url"), token, digest);
    cfg.template.vcpus = pinned_template().vcpus;
    cfg.template.mem_mib = pinned_template().mem_mib;
    FirecrackerOrchestrator::new(cfg).expect("client")
}

async fn live(tag: &str) -> (FakeAgent, PathBuf) {
    let token = token_file(tag, TOKEN);
    let agent = FakeAgent::serve(FakeHypervisor::new(0.8), &token).await;
    (agent, token)
}

fn spec(template: VmTemplate) -> TopicVmSpec {
    let req = request();
    TopicVmSpec::for_topic(&req.topic_id, template, req.sandbox)
}

/// Over the live wire: a topic whose params select an in-guest runner gets
/// one experiment VM per paid job on the agent (beside its RLM VM, sized by
/// the topic under the lock ceilings, carrying the pack pin), the host's
/// `experiment_vm` attestation stamps the report, and the VM is destroyed
/// after the job. A second evaluation is a second VM, and a topic that
/// selects nothing keeps the sister path.
#[tokio::test]
async fn an_experiment_topic_gets_one_attested_vm_per_paid_job_over_the_wire() {
    use proof_rlm::fixtures::experiment_request;
    use proof_vm_proto::GuestMode;
    let (agent, token) = live("experiments").await;
    let orch = Arc::new(client(&agent, &token, &pinned_template().image_digest));
    let runner = VmBackedRunner::new(orch.clone(), pinned_template());
    let req = experiment_request(Some(4));
    runner.inspect(&req, &rules()).await.expect("inspect");
    let hv = &agent.hypervisor;
    hv.set_rlm_flops(Some(77));
    let run = runner
        .evaluate(&req, &token_for(&req))
        .await
        .expect("evaluate in an experiment vm");
    assert!(run.report.sandboxed, "the host attested the experiment vm");
    assert_eq!(
        run.report.flops_used,
        Some(77),
        "the guest's measurement, host-relayed"
    );
    let boots = hv.boots();
    assert_eq!(boots.len(), 2, "topic vm + one experiment vm");
    let exp = hv
        .spec_of(&boots[1].vm_id)
        .expect("spec")
        .experiment
        .expect("experiment spec");
    assert_eq!(exp.runner, "placeholder_in_guest_runner");
    assert_eq!(exp.pack.digest, format!("sha256:{}", "ee".repeat(32)));
    assert_eq!(exp.disk_mib, 32_768);
    let spec = hv.spec_of(&boots[1].vm_id).expect("spec");
    assert_eq!(
        (spec.template.vcpus, spec.template.mem_mib),
        (4, 8_192),
        "the topic's 4 vCPU ask, the 8 GiB default"
    );
    assert!(boots[1].vm_id.contains("-x"), "{}", boots[1].vm_id);
    assert_eq!(
        hv.teardowns(),
        vec![(boots[1].vm_id.clone(), RetainPolicy::Destroy)],
        "destroyed after its one job"
    );
    let jobs = hv.jobs();
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        jobs[1].0, boots[1].vm_id,
        "the paid job ran in the experiment vm"
    );
    assert_eq!(agent.state.running_experiments().await, 0);
    assert_eq!(agent.state.running().await.len(), 1, "the topic vm stays");

    runner
        .evaluate(&req, &token_for(&req))
        .await
        .expect("second evaluate");
    assert_eq!(hv.boots().len(), 3, "a second experiment is a second vm");
    assert_eq!(hv.teardowns().len(), 2);

    // The host stops attesting: the report comes back unsandboxed and the
    // client refuses it for a firecracker_required topic — no substitute.
    hv.set_experiment_attests(false);
    let err = runner
        .evaluate(&req, &token_for(&req))
        .await
        .expect_err("no attestation");
    assert!(
        err.to_string()
            .contains("without the host's sister-guest attestation"),
        "{err}"
    );
    assert_eq!(hv.teardowns().len(), 3, "still destroyed after the failure");
    hv.set_experiment_attests(true);

    // The attestation must be about the VM the job was dispatched to.
    let handle = orch
        .attach(&req.topic_id)
        .await
        .expect("attach")
        .expect("topic vm");
    let mut plain = request();
    plain.constraints.params.clear();
    let out = orch
        .run(
            &handle,
            VmJob::Evaluate {
                request: plain.clone(),
                checklist_digest: token_for(&plain).checklist_digest().to_owned(),
                rules_version: 1,
            },
        )
        .await
        .expect("sister path on the topic vm");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("shape");
    };
    assert!(run.report.sandboxed);
    assert_eq!(GuestMode::Sister.network(), "none");
}

#[tokio::test]
async fn one_topic_one_vm_inspect_then_paid_run_with_sister_attestation() {
    let (agent, token) = live("flow").await;
    let orch = Arc::new(client(&agent, &token, &pinned_template().image_digest));
    orch.ready().expect("token + pin present");
    let health = orch.health().await.expect("health");
    assert!(health.ready);
    assert_eq!(health.hypervisor, "fake");

    let runner = VmBackedRunner::new(orch.clone(), pinned_template());
    runner.ready().expect("runner ready");
    let req = request();
    let inspected = runner.inspect(&req, &rules()).await.expect("inspect");
    assert!(inspected.checklist.is_green(&rules()));
    let run = runner
        .evaluate(&req, &token_for(&req))
        .await
        .expect("evaluate");
    assert!((run.report.primary_value - 0.8).abs() < 1e-12);
    assert!(run.report.sandboxed, "host attested the sister guest");
    assert_eq!(
        run.report.flops_used,
        Some(1),
        "the sister's measurement, not the RLM's"
    );
    let hv = &agent.hypervisor;
    assert_eq!(hv.boots().len(), 1, "second job attached, no second boot");
    assert_eq!(hv.boots()[0].topic_id, req.topic_id);
    assert_eq!(hv.boots()[0].image_digest, pinned_template().image_digest);
    let jobs = hv.jobs();
    assert_eq!(jobs.len(), 2);
    assert!(matches!(jobs[0].1, VmJob::Inspect { .. }));
    assert!(matches!(jobs[1].1, VmJob::Evaluate { .. }));
    for (_, job) in &jobs {
        let dump = serde_json::to_string(job).expect("json");
        for forbidden in [
            "/run/base",
            "/opt/base",
            "api_key",
            "127.0.0.1",
            "base_url",
            TOKEN,
        ] {
            assert!(!dump.contains(forbidden), "job leaked {forbidden}: {dump}");
        }
    }

    let handle = orch
        .attach(&req.topic_id)
        .await
        .expect("attach")
        .expect("exists");
    assert_eq!(handle.topic_id, req.topic_id);
    assert!(orch
        .teardown(&handle, RetainPolicy::Destroy)
        .await
        .expect("teardown"));
    assert_eq!(
        hv.teardowns(),
        vec![(handle.vm_id.clone(), RetainPolicy::Destroy)]
    );
    assert_eq!(orch.attach(&req.topic_id).await.expect("attach"), None);
    assert_eq!(
        orch.teardown(&handle, RetainPolicy::Destroy).await,
        Err(VmError::Backend(format!(
            "orchestrator knows no vm {}",
            handle.vm_id
        ))),
        "a destroyed vm is gone"
    );
}

#[tokio::test]
async fn missing_token_or_unpinned_digest_is_not_wired_and_never_calls_out() {
    let (agent, token) = live("unwired").await;
    let no_token = client(
        &agent,
        Path::new("/nonexistent/vm_orchestrator_token"),
        &pinned_template().image_digest,
    );
    let err = no_token.ready().expect_err("no token file");
    assert!(matches!(err, VmError::NotWired(_)), "{err}");
    assert!(
        err.to_string().contains(VM_ORCHESTRATOR_TOKEN_FILE_ENV),
        "{err}"
    );
    let runner = VmBackedRunner::new(Arc::new(no_token), pinned_template());
    assert!(matches!(
        runner.inspect(&request(), &rules()).await,
        Err(RunnerError::NotWired(_))
    ));

    let unpinned = client(&agent, &token, "");
    let err = unpinned.ready().expect_err("unpinned");
    assert!(matches!(err, VmError::NotWired(_)), "{err}");
    assert!(err.to_string().contains(RLM_VM_IMAGE_DIGEST_ENV), "{err}");
    let err = unpinned
        .create(&spec(VmTemplate::unpinned()))
        .await
        .expect_err("create refuses before any request");
    assert!(matches!(err, VmError::NotWired(_)), "{err}");
    assert!(
        agent.hypervisor.boots().is_empty(),
        "nothing reached the agent"
    );
    assert!(agent.hypervisor.jobs().is_empty());
}

#[tokio::test]
async fn a_wrong_bearer_or_a_dead_agent_is_a_backend_refusal_without_the_token() {
    let (agent, _) = live("bearer").await;
    let wrong = token_file("bearer-wrong", "another-token-not-a-real-secret");
    let orch = client(&agent, &wrong, &pinned_template().image_digest);
    orch.ready().expect("configured");
    let err = orch
        .create(&spec(pinned_template()))
        .await
        .expect_err("refused");
    let text = err.to_string();
    assert!(matches!(err, VmError::Backend(_)), "{text}");
    assert!(text.contains("bearer"), "{text}");
    assert!(!text.contains("another-token"), "token leaked: {text}");
    assert!(!text.contains(TOKEN), "token leaked: {text}");
    let runner = VmBackedRunner::new(Arc::new(orch), pinned_template());
    assert!(matches!(
        runner.inspect(&request(), &rules()).await,
        Err(RunnerError::Backend(_))
    ));

    let (dead, token) = live("dead").await;
    let orch = client(&dead, &token, &pinned_template().image_digest);
    dead.stop();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let err = orch
        .attach("topic-a")
        .await
        .expect_err("agent down is not None");
    assert!(matches!(err, VmError::Backend(_)), "{err}");
    assert!(err.to_string().contains("unreachable"), "{err}");
    // The text reaches a miner as a 503: the route, never the agent's address.
    assert!(
        err.to_string().contains("/v1/vms/by-topic/topic-a"),
        "{err}"
    );
    assert!(
        !err.to_string().contains(&dead.addr.port().to_string()),
        "agent url leaked: {err}"
    );
}

#[tokio::test]
async fn jobs_are_bound_to_the_handle_topic_before_any_request() {
    let (agent, token) = live("bind").await;
    let orch = client(&agent, &token, &pinned_template().image_digest);
    let handle = orch.create(&spec(pinned_template())).await.expect("create");
    let err = orch
        .run(
            &handle,
            VmJob::Archive {
                topic_id: "topic-b".into(),
            },
        )
        .await
        .expect_err("other topic");
    assert_eq!(err, VmError::Spec("topic_id"));
    assert!(agent.hypervisor.jobs().is_empty(), "never left the client");
    let forged = VmHandle {
        topic_id: "topic-b".into(),
        vm_id: handle.vm_id.clone(),
    };
    let err = orch
        .run(
            &forged,
            VmJob::Archive {
                topic_id: "topic-b".into(),
            },
        )
        .await
        .expect_err("agent refuses the mismatch");
    assert!(matches!(err, VmError::Backend(_)), "{err}");
    assert!(err.to_string().contains("TopicMismatch"), "{err}");
    let err = orch
        .teardown(&forged, RetainPolicy::Retain)
        .await
        .expect_err("teardown bound too");
    assert!(err.to_string().contains("TopicMismatch"), "{err}");
    assert!(agent.hypervisor.teardowns().is_empty());
    let out = orch
        .run(
            &handle,
            VmJob::Archive {
                topic_id: handle.topic_id.clone(),
            },
        )
        .await
        .expect("bound job runs");
    assert_eq!(out, VmJobOutput::Archived);
}

#[tokio::test]
async fn a_firecracker_required_run_without_the_sister_attestation_is_not_evidence() {
    let (agent, token) = live("sister").await;
    agent.hypervisor.set_sister(false);
    agent.hypervisor.set_rlm_claims_sandboxed(true);
    let orch = Arc::new(client(&agent, &token, &pinned_template().image_digest));
    let runner = VmBackedRunner::new(orch.clone(), pinned_template());
    let req = request();
    assert!(req.sandbox.firecracker_required);
    let err = runner
        .evaluate(&req, &token_for(&req))
        .await
        .expect_err("no sister");
    assert!(matches!(err, RunnerError::Backend(_)), "{err}");
    assert!(err.to_string().contains("sister"), "{err}");

    let mut relaxed = req.clone();
    relaxed.sandbox.firecracker_required = false;
    let handle = orch
        .attach(&req.topic_id)
        .await
        .expect("attach")
        .expect("vm");
    let out = orch
        .run(
            &handle,
            VmJob::Baseline {
                request: relaxed.clone(),
            },
        )
        .await
        .expect("a topic that does not require the guest may run in the RLM VM");
    let VmJobOutput::Baseline(report) = out else {
        panic!("shape");
    };
    assert!(!report.sandboxed, "the host overruled the RLM's claim");
    report
        .verify(&relaxed)
        .expect("not required, so still evidence");
    assert!(report.verify(&req).is_err());
}

/// Sister evidence is bound to the artefact it ran. A paid run for artefact
/// B that comes back with the attestation of artefact A is refused on both
/// sides: the agent answers 502 `evidence_mismatch` and never stamps, and a
/// client that received such a body would refuse it too. No row, no score.
#[tokio::test]
async fn replayed_sister_evidence_for_another_artifact_never_scores() {
    let (agent, token) = live("replay").await;
    let orch = Arc::new(client(&agent, &token, &pinned_template().image_digest));
    let runner = VmBackedRunner::new(orch.clone(), pinned_template());
    let a = request();
    let mut b = request();
    b.submission_digest = "submission-b".into();
    b.artifact_digest = "ba".repeat(32);
    agent
        .hypervisor
        .set_sister_replay(Some(proof_vm_proto::EvidenceBinding::new(
            &a.topic_id,
            &a.submission_digest,
            &a.artifact_digest,
        )));
    let err = runner
        .evaluate(&b, &token_for(&b))
        .await
        .expect_err("evidence for a is not evidence for b");
    assert!(matches!(err, RunnerError::Backend(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("EvidenceMismatch"), "{text}");
    assert!(text.contains("submission_digest"), "{text}");

    // The client's own check refuses the same body should an agent ever emit it.
    let handle = orch.attach(&b.topic_id).await.expect("attach").expect("vm");
    let sister = proof_vm_proto::SisterAttestation {
        mode: proof_vm_proto::GuestMode::Sister,
        sister_vm_id: format!("{}-s1", handle.vm_id),
        image_digest: format!("sha256:{}", "dd".repeat(32)),
        topic_id: a.topic_id.clone(),
        submission_digest: a.submission_digest.clone(),
        artifact_digest: a.artifact_digest.clone(),
        sandboxed: true,
        network: "none".into(),
        flops_used: Some(1),
        wall_ms: 1,
        exit_code: Some(0),
    };
    let job_b = VmJob::Baseline { request: b.clone() };
    let out_b = VmJobOutput::Baseline(proof_rlm::fixtures::report_for(&b, 0.5));
    assert!(proof_vm_proto::bind_evidence(&job_b, &out_b, Some(&sister)).is_err());

    agent.hypervisor.set_sister_replay(None);
    let run = runner
        .evaluate(&b, &token_for(&b))
        .await
        .expect("honest evidence for b scores b");
    assert!(run.report.sandboxed);
    assert_eq!(run.report.submission_digest, "submission-b");
}

#[tokio::test]
async fn the_created_vm_must_run_the_pinned_image_and_a_fake_answer_is_refused() {
    let (agent, token) = live("pin").await;
    let mut other = pinned_template();
    other.image_digest = format!("sha256:{}", "ee".repeat(32));
    let orch = client(&agent, &token, &other.image_digest);
    let handle = orch.create(&spec(other.clone())).await.expect("create");
    assert_eq!(agent.hypervisor.boots()[0].image_digest, other.image_digest);
    // A second create for the same topic is the agent's one-VM-per-topic rule.
    let err = orch.create(&spec(other)).await.expect_err("duplicate");
    assert!(err.to_string().contains("AlreadyExists"), "{err}");
    assert!(orch
        .teardown(&handle, RetainPolicy::Retain)
        .await
        .expect("retain"));
    assert_eq!(
        orch.attach(&handle.topic_id).await.expect("attach"),
        None,
        "a retained vm is not attachable; the next run creates a fresh one"
    );
    // The reference fake orchestrator and the live client agree on the contract.
    let reference = FakeOrchestrator::new(0.8);
    reference.ready().expect("reference");
}

#[tokio::test]
async fn from_env_is_none_when_unset_and_reads_the_locked_shape() {
    let _guard = ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for name in [
        VM_ORCHESTRATOR_URL_ENV,
        VM_ORCHESTRATOR_TOKEN_FILE_ENV,
        RLM_VM_IMAGE_DIGEST_ENV,
        RLM_VM_VCPUS_ENV,
        RLM_VM_MEM_MIB_ENV,
    ] {
        std::env::remove_var(name);
    }
    assert!(FirecrackerOrchestrator::from_env()
        .expect("unset is fine")
        .is_none());

    std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "https://kvm.example.invalid:8200");
    assert_eq!(
        FcConfig::from_env().expect_err("token file env required"),
        FcConfigError::NoTokenFile
    );
    std::env::set_var(
        VM_ORCHESTRATOR_TOKEN_FILE_ENV,
        "/run/base/proof/vm_orchestrator_token",
    );
    let cfg = FcConfig::from_env().expect("config").expect("some");
    assert_eq!(cfg.template.vcpus, DEFAULT_RLM_VCPUS);
    assert_eq!(cfg.template.mem_mib, DEFAULT_RLM_MEM_MIB);
    assert!(
        cfg.template.image_digest.is_empty(),
        "unpinned until the operator sets it"
    );
    let orch = FirecrackerOrchestrator::from_env()
        .expect("builds")
        .expect("some");
    let err = orch.ready().expect_err("no token file on this box, no pin");
    assert!(matches!(err, VmError::NotWired(_)), "{err}");
    assert!(!format!("{orch:?}").contains("Bearer"));

    std::env::set_var(
        RLM_VM_IMAGE_DIGEST_ENV,
        format!("sha256:{}", "ab".repeat(32)),
    );
    std::env::set_var(RLM_VM_VCPUS_ENV, "8");
    std::env::set_var(RLM_VM_MEM_MIB_ENV, "16384");
    let cfg = FcConfig::from_env().expect("config").expect("some");
    assert_eq!((cfg.template.vcpus, cfg.template.mem_mib), (8, 16_384));
    cfg.template.validate().expect("pinned");
    std::env::set_var(RLM_VM_VCPUS_ENV, "many");
    assert!(matches!(
        FcConfig::from_env(),
        Err(FcConfigError::BadNumber(RLM_VM_VCPUS_ENV, _))
    ));
    std::env::remove_var(RLM_VM_VCPUS_ENV);
    std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "http://10.0.0.9:8200");
    assert!(matches!(
        FcConfig::from_env(),
        Err(FcConfigError::Insecure(_))
    ));
    std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "not a url");
    assert!(matches!(
        FcConfig::from_env(),
        Err(FcConfigError::BadUrl(_))
    ));
    for name in [
        VM_ORCHESTRATOR_URL_ENV,
        VM_ORCHESTRATOR_TOKEN_FILE_ENV,
        RLM_VM_IMAGE_DIGEST_ENV,
        RLM_VM_MEM_MIB_ENV,
    ] {
        std::env::remove_var(name);
    }
}
