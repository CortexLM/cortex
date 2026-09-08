//! `proof-vm-orchestrator` agent library.
//!
//! The agent runs on a host with a working `/dev/kvm` — **dedicated DO
//! metal** in production (never colocated on the control plane); on staging
//! the control-plane droplet itself with nested `/dev/kvm` is an allowed,
//! proven exception; never a Lium pod — and is the only thing that talks to
//! Firecracker.
//! The Proof control plane reaches it over HTTPS with a bearer read from a
//! file ([`BearerAuth`]) and drives four verbs (`proof_vm_proto::paths`):
//!
//! | Verb | Route | Bind |
//! |------|-------|------|
//! | create | `POST /v1/vms` | one running VM per `topic_id`; a second create is 409 |
//! | attach | `GET /v1/vms/by-topic/{topic_id}` | 404 when the topic has no running VM |
//! | run | `POST /v1/vms/{vm_id}/jobs` | request `topic_id` **and** the job's own topic must equal the VM's |
//! | teardown | `DELETE /v1/vms/{vm_id}` | request `topic_id` must equal the VM's; destroy or retain |
//!
//! "Running" means the hypervisor confirms the process is alive: a VM that
//! died outside a teardown is reaped per its retain policy, recorded as
//! `crashed`, and its topic may create a fresh one.
//!
//! The agent never mounts a host path into a guest, never receives a key
//! from the control plane, and stamps `sandboxed` / `flops_used` on paid
//! outputs from the sister guest **it** booted ([`stamp_output`]) — and only
//! after `proof_vm_proto::bind_evidence` confirmed the attestation and the
//! report name that job's topic, submission, and artefact. The
//! [`Hypervisor`] behind it is Firecracker + jailer in production
//! (`proof-fc-host`) and [`fixtures::FakeHypervisor`] in every test — no
//! test here or in CI boots a VM.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

mod auth;
mod hypervisor;
mod router;
mod stamp;

/// Fake hypervisor + in-process agent. Test builds and the `test-fixtures`
/// feature only; never part of a host binary.
#[cfg(any(test, feature = "test-fixtures"))]
#[path = "fixtures_tests.rs"]
pub mod fixtures;

pub use auth::{AuthError, BearerAuth};
pub use hypervisor::{BootedVm, HvError, Hypervisor, JobOutcome};
pub use router::{agent_router, AgentError, AgentState};
pub use stamp::{output_matches, stamp_output};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use proof_rlm::fixtures::{pinned_template, request, rules, token_for};
    use proof_rlm::{RetainPolicy, TopicVmSpec, VmJob, VmJobOutput};
    use proof_vm_proto::{
        paths, AgentHealth, CreateVmRequest, ErrorBody, ErrorCode, RunJobRequest, RunJobResponse,
        TeardownRequest, TeardownResponse, VmRecord, VmState,
    };
    use tower::ServiceExt;

    use super::fixtures::{token_file, FakeHypervisor};
    use super::*;

    const TOKEN: &str = "agent-test-token-not-a-real-secret";

    fn app(hv: Arc<FakeHypervisor>, tag: &str) -> (axum::Router, AgentState) {
        let auth = Arc::new(BearerAuth::from_file(&token_file(tag, TOKEN)));
        let state = AgentState::new(hv, auth);
        (agent_router(state.clone()), state)
    }

    async fn call<T: serde::de::DeserializeOwned>(
        app: &axum::Router,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, T) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(b) = bearer {
            req = req.header("authorization", format!("Bearer {b}"));
        }
        let req = match body {
            Some(v) => req
                .header("content-type", "application/json")
                .body(Body::from(v.to_string())),
            None => req.body(Body::empty()),
        }
        .expect("request");
        let resp = app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let parsed = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{status} body {:?}: {e}", String::from_utf8_lossy(&bytes)));
        (status, parsed)
    }

    fn spec() -> TopicVmSpec {
        let req = request();
        TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox)
    }

    async fn create(app: &axum::Router) -> VmRecord {
        let (status, rec): (StatusCode, VmRecord) = call(
            app,
            "POST",
            paths::VMS,
            Some(TOKEN),
            Some(serde_json::to_value(CreateVmRequest { spec: spec() }).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        rec
    }

    #[tokio::test]
    async fn every_route_needs_the_bearer_including_health() {
        let (app, _) = app(FakeHypervisor::new(0.5), "auth");
        for (method, path) in [
            ("GET", paths::HEALTH.to_owned()),
            ("POST", paths::VMS.to_owned()),
            ("GET", paths::vm_by_topic("topic-a")),
            ("POST", paths::vm_jobs("x")),
            ("DELETE", paths::vm("x")),
        ] {
            let (status, err): (StatusCode, ErrorBody) =
                call(&app, method, &path, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert_eq!(err.code, ErrorCode::Unauthorized);
            let (status, _): (StatusCode, ErrorBody) =
                call(&app, method, &path, Some("wrong-token"), None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        }
        let (status, health): (StatusCode, AgentHealth) =
            call(&app, "GET", paths::HEALTH, Some(TOKEN), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(health.ready);
        assert_eq!(health.hypervisor, "fake");
        assert_eq!(health.vms, 0);
    }

    #[tokio::test]
    async fn one_vm_per_topic_then_attach_run_and_destroy() {
        let hv = FakeHypervisor::new(0.8);
        let (app, state) = app(hv.clone(), "flow");
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(err.code, ErrorCode::NotFound);

        let rec = create(&app).await;
        assert_eq!(rec.handle.topic_id, "topic-a");
        assert!(
            rec.handle.vm_id.starts_with("topic-a-"),
            "{}",
            rec.handle.vm_id
        );
        assert_eq!(rec.state, VmState::Running);
        assert_eq!(rec.image_digest, pinned_template().image_digest);
        assert_eq!((rec.vcpus, rec.mem_mib), (2, 4_096));
        assert_eq!(hv.boots().len(), 1);

        let (status, dup): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            paths::VMS,
            Some(TOKEN),
            Some(serde_json::to_value(CreateVmRequest { spec: spec() }).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(dup.code, ErrorCode::AlreadyExists);
        assert_eq!(hv.boots().len(), 1, "no second boot");

        let (status, attached): (StatusCode, VmRecord) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(attached, rec);
        assert_eq!(state.running().await.len(), 1);
    }

    #[tokio::test]
    async fn a_job_runs_on_the_bound_vm_and_destroy_removes_it() {
        let hv = FakeHypervisor::new(0.8);
        let (app, _) = app(hv.clone(), "destroy");
        let rec = create(&app).await;
        let req = request();
        let job = VmJob::Inspect {
            request: req.clone(),
            rules: rules(),
        };
        let (status, out): (StatusCode, RunJobResponse) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: req.topic_id.clone(),
                    job,
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(out.vm_id, rec.handle.vm_id);
        assert!(out.sister.is_none(), "inspection boots no sister");
        assert!(matches!(out.output, VmJobOutput::Inspected(_)));

        let (status, down): (StatusCode, TeardownResponse) = call(
            &app,
            "DELETE",
            &paths::vm(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(TeardownRequest {
                    topic_id: "topic-a".into(),
                    policy: RetainPolicy::Destroy,
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(down.confirmed);
        assert_eq!(down.state, VmState::Destroyed);
        assert_eq!(
            hv.teardowns(),
            vec![(rec.handle.vm_id.clone(), RetainPolicy::Destroy)]
        );
        let (status, _): (StatusCode, ErrorBody) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "destroyed vms are gone");
        let (status, _): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: req.topic_id.clone(),
                    job: VmJob::Archive {
                        topic_id: req.topic_id.clone(),
                    },
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// A job or teardown that names another topic than the VM's never reaches
    /// the hypervisor — whether the mismatch is in the envelope or the job.
    #[tokio::test]
    async fn the_topic_bind_is_hard_on_both_the_envelope_and_the_job() {
        let hv = FakeHypervisor::new(0.8);
        let (app, _) = app(hv.clone(), "bind");
        let rec = create(&app).await;
        let req = request();
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: "topic-b".into(),
                    job: VmJob::Archive {
                        topic_id: req.topic_id.clone(),
                    },
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(err.code, ErrorCode::TopicMismatch);
        let mut other = req.clone();
        other.topic_id = "topic-b".into();
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: req.topic_id.clone(),
                    job: VmJob::Baseline { request: other },
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(err.code, ErrorCode::TopicMismatch);
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "DELETE",
            &paths::vm(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(TeardownRequest {
                    topic_id: "topic-b".into(),
                    policy: RetainPolicy::Destroy,
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(err.code, ErrorCode::TopicMismatch);
        assert!(hv.jobs().is_empty(), "nothing reached the hypervisor");
        assert!(hv.teardowns().is_empty());
    }

    /// Paid outputs carry the host's facts: the RLM's `sandboxed` claim is
    /// replaced by whether a sister booted, and `flops_used` by what the
    /// sister measured.
    #[tokio::test]
    async fn paid_outputs_are_host_stamped_and_carry_the_attestation() {
        let hv = FakeHypervisor::new(0.8);
        hv.set_rlm_claims_sandboxed(true);
        hv.set_rlm_flops(Some(123));
        hv.set_sister_flops(Some(7));
        let (app, _) = app(hv.clone(), "stamp");
        let rec = create(&app).await;
        let req = request();
        let evaluate = |req: &proof_rlm::CustomRunRequest| RunJobRequest {
            topic_id: req.topic_id.clone(),
            job: VmJob::Evaluate {
                request: req.clone(),
                checklist_digest: token_for(req).checklist_digest().to_owned(),
                rules_version: 1,
            },
        };
        let (status, out): (StatusCode, RunJobResponse) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(serde_json::to_value(evaluate(&req)).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sister = out.sister.expect("sister attested");
        assert!(sister.sandboxed);
        assert_eq!(sister.network, "none");
        let VmJobOutput::Evaluated(run) = out.output else {
            panic!("shape");
        };
        assert!(run.report.sandboxed);
        assert_eq!(run.report.flops_used, Some(7), "sister measurement wins");
        run.report.verify(&req).expect("bound");

        hv.set_sister(false);
        let (status, out): (StatusCode, RunJobResponse) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(serde_json::to_value(evaluate(&req)).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(out.sister.is_none());
        let VmJobOutput::Evaluated(run) = out.output else {
            panic!("shape");
        };
        assert!(
            !run.report.sandboxed,
            "no sister, the RLM's claim is overruled"
        );
        assert_eq!(run.report.flops_used, Some(123), "the RLM ran it itself");
        assert!(matches!(
            run.report.verify(&req),
            Err(proof_rlm::ReportError::NotSandboxed)
        ));
    }

    /// Sister evidence is bound to the job it was produced for. A hypervisor
    /// (or a compromised guest behind it) that presents the attestation of
    /// artefact A for a paid job on artefact B gets a 502 and nothing is
    /// stamped; so does a report that names another submission than the job.
    #[tokio::test]
    async fn replayed_sister_evidence_for_another_artifact_is_refused() {
        let hv = FakeHypervisor::new(0.8);
        let (app, _) = app(hv.clone(), "replay");
        let rec = create(&app).await;
        let a = request();
        let mut b = request();
        b.submission_digest = "submission-b".into();
        b.artifact_digest = "ba".repeat(32);
        let evaluate = |req: &proof_rlm::CustomRunRequest| {
            serde_json::to_value(RunJobRequest {
                topic_id: req.topic_id.clone(),
                job: VmJob::Evaluate {
                    request: req.clone(),
                    checklist_digest: token_for(req).checklist_digest().to_owned(),
                    rules_version: 1,
                },
            })
            .expect("json")
        };
        hv.set_sister_replay(Some(proof_vm_proto::EvidenceBinding::new(
            &a.topic_id,
            &a.submission_digest,
            &a.artifact_digest,
        )));
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(evaluate(&b)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(err.code, ErrorCode::EvidenceMismatch);
        assert!(err.error.contains("submission_digest"), "{}", err.error);
        assert!(err.error.contains("submission-b"), "{}", err.error);

        hv.set_sister_replay(None);
        hv.set_rlm_report_submission(Some("submission-c"));
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(evaluate(&b)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(err.code, ErrorCode::EvidenceMismatch);
        assert!(err.error.starts_with("report names"), "{}", err.error);

        hv.set_rlm_report_submission(None);
        let (status, out): (StatusCode, RunJobResponse) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(evaluate(&b)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sister = out.sister.expect("honest sister");
        assert_eq!(sister.submission_digest, "submission-b");
        assert_eq!(sister.artifact_digest, "ba".repeat(32));
        assert_eq!(sister.topic_id, b.topic_id);
        let VmJobOutput::Evaluated(run) = out.output else {
            panic!("shape");
        };
        assert!(run.report.sandboxed);
        run.report.verify(&b).expect("bound to b");
    }

    /// A VM whose process exits outside a teardown is not advertised as
    /// running: attach is 404, a job on it is 404 (no dispatch), the host is
    /// asked to release it per the retain policy, and the topic may create a
    /// fresh VM instead of being blocked by the dead one.
    #[tokio::test]
    async fn a_dead_vm_is_reaped_and_its_topic_can_recreate() {
        let hv = FakeHypervisor::new(0.8);
        let (app, state) = app(hv.clone(), "dead");
        let rec = create(&app).await;
        assert_eq!(state.running().await.len(), 1);
        hv.kill(&rec.handle.vm_id);

        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "a dead vm is not attachable");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(
            hv.teardowns(),
            vec![(rec.handle.vm_id.clone(), RetainPolicy::Destroy)],
            "reaped once, per the record's retain policy"
        );
        assert!(state.running().await.is_empty());

        let req = request();
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: req.topic_id.clone(),
                    job: VmJob::Archive {
                        topic_id: req.topic_id.clone(),
                    },
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(err.error.contains("Crashed"), "{}", err.error);
        assert!(hv.jobs().is_empty(), "no job reaches a dead vm");

        let fresh = create(&app).await;
        assert_ne!(fresh.handle.vm_id, rec.handle.vm_id);
        assert_eq!(fresh.state, VmState::Running);
        assert_eq!(hv.boots().len(), 2);
        let (status, attached): (StatusCode, VmRecord) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(attached, fresh);
        assert_eq!(hv.teardowns().len(), 1, "the fresh vm is not reaped");

        // Teardown of the crashed record is idempotent and confirmed.
        let (status, down): (StatusCode, TeardownResponse) = call(
            &app,
            "DELETE",
            &paths::vm(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(TeardownRequest {
                    topic_id: "topic-a".into(),
                    policy: RetainPolicy::Destroy,
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(down.state, VmState::Crashed);
        assert!(down.confirmed);
        assert_eq!(hv.teardowns().len(), 1, "not released twice");
    }

    /// The process dies while a job is in flight: the job fails, the VM is
    /// reaped on the way out (the job holds the lock), and the next create
    /// for the topic boots a fresh VM without a 409. Health sweeps too.
    #[tokio::test]
    async fn a_vm_that_dies_under_a_job_is_reaped_by_that_job() {
        let hv = FakeHypervisor::new(0.8);
        let (app, state) = app(hv.clone(), "dies-under-job");
        let rec = create(&app).await;
        hv.set_dies_under_job(true);
        let req = request();
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(RunJobRequest {
                    topic_id: req.topic_id.clone(),
                    job: VmJob::Archive {
                        topic_id: req.topic_id.clone(),
                    },
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(err.code, ErrorCode::Backend);
        assert_eq!(hv.jobs().len(), 1, "the job was dispatched and failed");
        assert_eq!(
            hv.teardowns(),
            vec![(rec.handle.vm_id.clone(), RetainPolicy::Destroy)],
            "reaped by the failing job"
        );
        assert!(state.running().await.is_empty());
        let fresh = create(&app).await;
        assert_eq!(fresh.handle.topic_id, "topic-a");
        let (_, health): (StatusCode, AgentHealth) =
            call(&app, "GET", paths::HEALTH, Some(TOKEN), None).await;
        assert_eq!(health.vms, 2, "the crashed record stays for audit");
        hv.kill(&fresh.handle.vm_id);
        let (_, health): (StatusCode, AgentHealth) =
            call(&app, "GET", paths::HEALTH, Some(TOKEN), None).await;
        assert_eq!(health.vms, 2);
        assert_eq!(hv.teardowns().len(), 2, "health sweeps the dead too");
        assert!(state.running().await.is_empty());
    }

    #[tokio::test]
    async fn a_busy_vm_refuses_a_second_job_and_retain_keeps_the_record() {
        let hv = FakeHypervisor::new(0.8);
        hv.set_job_delay(Some(std::time::Duration::from_millis(300)));
        let (app, state) = app(hv.clone(), "busy");
        let rec = create(&app).await;
        let req = request();
        let body = serde_json::to_value(RunJobRequest {
            topic_id: req.topic_id.clone(),
            job: VmJob::Archive {
                topic_id: req.topic_id.clone(),
            },
        })
        .expect("json");
        let slow = {
            let app = app.clone();
            let body = body.clone();
            let path = paths::vm_jobs(&rec.handle.vm_id);
            tokio::spawn(async move {
                let (status, _): (StatusCode, RunJobResponse) =
                    call(&app, "POST", &path, Some(TOKEN), Some(body)).await;
                status
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            &paths::vm_jobs(&rec.handle.vm_id),
            Some(TOKEN),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(slow.await.expect("join"), StatusCode::OK);

        let (status, down): (StatusCode, TeardownResponse) = call(
            &app,
            "DELETE",
            &paths::vm(&rec.handle.vm_id),
            Some(TOKEN),
            Some(
                serde_json::to_value(TeardownRequest {
                    topic_id: "topic-a".into(),
                    policy: RetainPolicy::Retain,
                })
                .expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(down.state, VmState::Retained);
        assert!(state.running().await.is_empty(), "retained is not running");
        let (status, _): (StatusCode, ErrorBody) = call(
            &app,
            "GET",
            &paths::vm_by_topic("topic-a"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a retained vm is not attachable"
        );
        let rec2 = create(&app).await;
        assert_ne!(
            rec2.handle.vm_id, rec.handle.vm_id,
            "fresh vm id after retain"
        );
    }

    #[tokio::test]
    async fn bad_specs_and_an_unready_hypervisor_never_boot() {
        let hv = FakeHypervisor::new(0.8);
        let (app, _) = app(hv.clone(), "spec");
        let mut unpinned = spec();
        unpinned.template.image_digest.clear();
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            paths::VMS,
            Some(TOKEN),
            Some(serde_json::to_value(CreateVmRequest { spec: unpinned }).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, ErrorCode::BadSpec);
        hv.set_ready(false);
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            paths::VMS,
            Some(TOKEN),
            Some(serde_json::to_value(CreateVmRequest { spec: spec() }).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, ErrorCode::NotReady);
        let (_, health): (StatusCode, AgentHealth) =
            call(&app, "GET", paths::HEALTH, Some(TOKEN), None).await;
        assert!(!health.ready);
        assert!(health.reason.contains("refuse"));
        hv.set_ready(true);
        hv.set_fail_boot(true);
        let (status, err): (StatusCode, ErrorBody) = call(
            &app,
            "POST",
            paths::VMS,
            Some(TOKEN),
            Some(serde_json::to_value(CreateVmRequest { spec: spec() }).expect("json")),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(err.code, ErrorCode::Backend);
        assert!(hv.boots().is_empty());
    }
}
