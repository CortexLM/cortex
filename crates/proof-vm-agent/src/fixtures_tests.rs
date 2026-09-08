//! Test fixtures: a recording fake [`Hypervisor`] and an in-process agent
//! server. No process is spawned, no VM boots — this is what CI runs.
//! Compiled for tests and the `test-fixtures` feature only.

// Test-only code: never compiled into a host binary (see the cfg in lib.rs).
#![allow(
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::expect_used,
    clippy::unwrap_used
)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use proof_canon::ChecklistRule;
use proof_rlm::fixtures::report_for;
use proof_rlm::{
    ArtifactFile, Checklist, CustomRunRequest, InspectOutcome, LogFile, RetainPolicy, RunOutcome,
    TopicVmSpec, VmJob, VmJobOutput,
};
use proof_vm_proto::SisterAttestation;

use crate::auth::BearerAuth;
use crate::hypervisor::{BootedVm, HvError, Hypervisor, JobOutcome};
use crate::router::{agent_router, AgentState};

/// Digest of the fake miner-guest image.
pub fn miner_image_digest() -> String {
    format!("sha256:{}", "dd".repeat(32))
}

/// Records every call; answers with canned documents.
pub struct FakeHypervisor {
    ready: AtomicBool,
    fail_boot: AtomicBool,
    primary: Mutex<f64>,
    red: Mutex<Option<String>>,
    /// Whether paid jobs boot a sister guest (the host's view).
    sister: AtomicBool,
    /// What that sister measures.
    sister_flops: Mutex<Option<u64>>,
    /// What the RLM writes into its own report before the host stamps it.
    rlm_claims_sandboxed: AtomicBool,
    rlm_flops: Mutex<Option<u64>>,
    proposed: Mutex<Vec<ChecklistRule>>,
    job_delay: Mutex<Option<Duration>>,
    boots: Mutex<Vec<BootedVm>>,
    jobs: Mutex<Vec<(String, VmJob)>>,
    teardowns: Mutex<Vec<(String, RetainPolicy)>>,
}

impl FakeHypervisor {
    pub fn new(primary: f64) -> Arc<Self> {
        Arc::new(Self {
            ready: AtomicBool::new(true),
            fail_boot: AtomicBool::new(false),
            primary: Mutex::new(primary),
            red: Mutex::new(None),
            sister: AtomicBool::new(true),
            sister_flops: Mutex::new(Some(1)),
            rlm_claims_sandboxed: AtomicBool::new(false),
            rlm_flops: Mutex::new(Some(999_999)),
            proposed: Mutex::new(vec![ChecklistRule {
                id: "rlm_rule".into(),
                text: "a rule the fake rlm wrote".into(),
            }]),
            job_delay: Mutex::new(None),
            boots: Mutex::new(Vec::new()),
            jobs: Mutex::new(Vec::new()),
            teardowns: Mutex::new(Vec::new()),
        })
    }

    pub fn set_ready(&self, v: bool) {
        self.ready.store(v, Ordering::SeqCst);
    }

    pub fn set_fail_boot(&self, v: bool) {
        self.fail_boot.store(v, Ordering::SeqCst);
    }

    pub fn set_primary(&self, v: f64) {
        *self.primary.lock().unwrap() = v;
    }

    /// Make inspections fail this rule id (None = green).
    pub fn set_red(&self, id: Option<&str>) {
        *self.red.lock().unwrap() = id.map(str::to_owned);
    }

    /// Whether the host boots a sister for paid jobs.
    pub fn set_sister(&self, v: bool) {
        self.sister.store(v, Ordering::SeqCst);
    }

    pub fn set_sister_flops(&self, v: Option<u64>) {
        *self.sister_flops.lock().unwrap() = v;
    }

    /// What the RLM claims before the host corrects it.
    pub fn set_rlm_claims_sandboxed(&self, v: bool) {
        self.rlm_claims_sandboxed.store(v, Ordering::SeqCst);
    }

    pub fn set_rlm_flops(&self, v: Option<u64>) {
        *self.rlm_flops.lock().unwrap() = v;
    }

    pub fn set_proposed(&self, rules: Vec<ChecklistRule>) {
        *self.proposed.lock().unwrap() = rules;
    }

    /// Make every job take this long (to exercise `Busy`).
    pub fn set_job_delay(&self, d: Option<Duration>) {
        *self.job_delay.lock().unwrap() = d;
    }

    pub fn boots(&self) -> Vec<BootedVm> {
        self.boots.lock().unwrap().clone()
    }

    pub fn jobs(&self) -> Vec<(String, VmJob)> {
        self.jobs.lock().unwrap().clone()
    }

    pub fn teardowns(&self) -> Vec<(String, RetainPolicy)> {
        self.teardowns.lock().unwrap().clone()
    }

    fn rlm_report(&self, req: &CustomRunRequest) -> proof_rlm::CustomRunReport {
        let mut r = report_for(req, *self.primary.lock().unwrap());
        r.sandboxed = self.rlm_claims_sandboxed.load(Ordering::SeqCst);
        r.flops_used = *self.rlm_flops.lock().unwrap();
        r
    }

    fn sister_for(&self, vm: &BootedVm, req: &CustomRunRequest) -> Option<SisterAttestation> {
        if !self.sister.load(Ordering::SeqCst) {
            return None;
        }
        Some(SisterAttestation {
            sister_vm_id: format!("{}-s{}", vm.vm_id, req.submission_digest.len()),
            image_digest: miner_image_digest(),
            sandboxed: true,
            network: "none".into(),
            flops_used: *self.sister_flops.lock().unwrap(),
            wall_ms: 10,
            exit_code: Some(0),
        })
    }
}

#[async_trait]
impl Hypervisor for FakeHypervisor {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn ready(&self) -> Result<(), HvError> {
        if self.ready.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HvError::NotReady("fake hypervisor told to refuse".into()))
        }
    }

    async fn boot(&self, vm_id: &str, spec: &TopicVmSpec) -> Result<BootedVm, HvError> {
        if self.fail_boot.load(Ordering::SeqCst) {
            return Err(HvError::Backend("fake boot failure".into()));
        }
        let vm = BootedVm {
            vm_id: vm_id.to_owned(),
            topic_id: spec.topic_id.clone(),
            image_digest: spec.template.image_digest.clone(),
        };
        self.boots.lock().unwrap().push(vm.clone());
        Ok(vm)
    }

    async fn run_job(&self, vm: &BootedVm, job: &VmJob) -> Result<JobOutcome, HvError> {
        self.jobs
            .lock()
            .unwrap()
            .push((vm.vm_id.clone(), job.clone()));
        let delay = *self.job_delay.lock().unwrap();
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        Ok(match job {
            VmJob::ProposeRules { .. } => JobOutcome {
                output: VmJobOutput::Rules(self.proposed.lock().unwrap().clone()),
                sister: None,
            },
            VmJob::Baseline { request } => JobOutcome {
                output: VmJobOutput::Baseline(self.rlm_report(request)),
                sister: self.sister_for(vm, request),
            },
            VmJob::Inspect { request, rules } => {
                let red = self.red.lock().unwrap().clone();
                let mut checklist =
                    Checklist::new(rules, &request.submission_digest, &request.artifact_digest);
                for r in &rules.rules {
                    checklist.record(&r.id, red.as_deref() != Some(r.id.as_str()), &r.text);
                }
                JobOutcome {
                    output: VmJobOutput::Inspected(InspectOutcome {
                        checklist,
                        artifact: vec![ArtifactFile {
                            path: "src/main.rs".into(),
                            bytes: b"fn main() {}\n".to_vec(),
                        }],
                    }),
                    sister: None,
                }
            }
            VmJob::Evaluate { request, .. } => JobOutcome {
                output: VmJobOutput::Evaluated(RunOutcome {
                    report: self.rlm_report(request),
                    logs: vec![LogFile {
                        name: "run.log".into(),
                        bytes: b"ok\n".to_vec(),
                    }],
                }),
                sister: self.sister_for(vm, request),
            },
            VmJob::Archive { .. } => JobOutcome {
                output: VmJobOutput::Archived,
                sister: None,
            },
        })
    }

    async fn teardown(&self, vm: &BootedVm, policy: RetainPolicy) -> Result<bool, HvError> {
        self.teardowns
            .lock()
            .unwrap()
            .push((vm.vm_id.clone(), policy));
        Ok(true)
    }
}

/// Write `token` to a fresh temp file and return its path.
pub fn token_file(tag: &str, token: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("proof-vm-agent-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("token");
    std::fs::write(&path, format!("{token}\n")).expect("write");
    path
}

/// A running in-process agent on a loopback port.
pub struct FakeAgent {
    /// Where it listens (`http://127.0.0.1:port`).
    pub addr: SocketAddr,
    /// The fake behind it.
    pub hypervisor: Arc<FakeHypervisor>,
    /// Shared state (for assertions).
    pub state: AgentState,
    task: tokio::task::JoinHandle<()>,
}

impl FakeAgent {
    /// Serve the agent router over `hypervisor`, authenticating against `token_file`.
    pub async fn serve(hypervisor: Arc<FakeHypervisor>, token_file: &Path) -> Self {
        let state = AgentState::new(
            hypervisor.clone(),
            Arc::new(BearerAuth::from_file(token_file)),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let app = agent_router(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            addr,
            hypervisor,
            state,
            task,
        }
    }

    /// `http://127.0.0.1:port`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Stop serving (later requests fail to connect).
    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}
