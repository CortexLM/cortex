//! The agent's HTTP surface: create / attach / run / teardown, bearer-gated,
//! with the topic ↔ VM bind enforced on every call that names a VM.
//!
//! A `Running` record is only advertised while the hypervisor confirms the
//! VM's process is alive: attach, create, run, and health probe it first, and
//! a VM whose process exited outside a teardown is **reaped** — released per
//! its retain policy and recorded as [`VmState::Crashed`] — so its topic can
//! get a fresh VM instead of being blocked by a dead one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use proof_rlm::{RetainPolicy, VmHandle};
use proof_vm_proto::{
    bind_evidence, paths, AgentHealth, CreateVmRequest, ErrorBody, ErrorCode, RunJobRequest,
    RunJobResponse, TeardownRequest, TeardownResponse, VmRecord, VmState, API_VERSION,
};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::auth::BearerAuth;
use crate::hypervisor::{BootedVm, HvError, Hypervisor};
use crate::stamp::{output_matches, stamp_output};

/// Longest `vm_id` the agent mints (jailer ids are capped at 64 chars).
const MAX_VM_ID_LEN: usize = 63;

/// One JSON error answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentError {
    /// Class (also the status).
    pub code: ErrorCode,
    /// Detail. Never a secret.
    pub error: String,
}

impl AgentError {
    fn new(code: ErrorCode, error: impl Into<String>) -> Self {
        Self {
            code,
            error: error.into(),
        }
    }
}

impl From<HvError> for AgentError {
    fn from(e: HvError) -> Self {
        let code = match e {
            HvError::NotReady(_) | HvError::Image(_) => ErrorCode::NotReady,
            HvError::Spec(_) => ErrorCode::BadSpec,
            HvError::Guest(_)
            | HvError::Backend(_)
            | HvError::Deadline(_)
            | HvError::Cancelled(_) => ErrorCode::Backend,
        };
        Self::new(code, e.to_string())
    }
}

impl IntoResponse for AgentError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            Json(ErrorBody {
                code: self.code,
                error: self.error,
            }),
        )
            .into_response()
    }
}

struct VmEntry {
    record: VmRecord,
    booted: BootedVm,
    /// One job (or teardown) at a time per VM.
    lock: Arc<Mutex<()>>,
}

struct Inner {
    hypervisor: Arc<dyn Hypervisor>,
    auth: Arc<BearerAuth>,
    vms: RwLock<BTreeMap<String, VmEntry>>,
    create_lock: Mutex<()>,
    next_id: AtomicU64,
}

/// Shared agent state.
#[derive(Clone)]
pub struct AgentState {
    inner: Arc<Inner>,
}

impl AgentState {
    /// State over `hypervisor`, gated by `auth`.
    #[must_use]
    pub fn new(hypervisor: Arc<dyn Hypervisor>, auth: Arc<BearerAuth>) -> Self {
        Self {
            inner: Arc::new(Inner {
                hypervisor,
                auth,
                vms: RwLock::new(BTreeMap::new()),
                create_lock: Mutex::new(()),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// VMs that are running **and** whose process is alive right now. Dead
    /// ones found on the way are reaped.
    pub async fn running(&self) -> Vec<VmRecord> {
        self.sweep().await
    }

    /// Probe every `Running` record; reap the dead. Returns the live ones.
    pub async fn sweep(&self) -> Vec<VmRecord> {
        let ids: Vec<String> = self
            .inner
            .vms
            .read()
            .await
            .values()
            .filter(|e| e.record.state == VmState::Running)
            .map(|e| e.record.handle.vm_id.clone())
            .collect();
        let mut live = Vec::new();
        for id in ids {
            if let Some(rec) = self.probe(&id).await {
                if rec.state == VmState::Running {
                    live.push(rec);
                }
            }
        }
        live
    }

    fn mint_vm_id(&self, topic_id: &str) -> String {
        let n = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let suffix = format!("-{n:04}");
        let keep = MAX_VM_ID_LEN.saturating_sub(suffix.len());
        let mut prefix = topic_id.to_owned();
        prefix.truncate(keep);
        format!("{prefix}{suffix}")
    }

    async fn entry(&self, vm_id: &str) -> Result<(VmRecord, BootedVm, Arc<Mutex<()>>), AgentError> {
        let vms = self.inner.vms.read().await;
        let e = vms
            .get(vm_id)
            .ok_or_else(|| AgentError::new(ErrorCode::NotFound, format!("no vm {vm_id}")))?;
        Ok((e.record.clone(), e.booted.clone(), e.lock.clone()))
    }

    /// The record for `vm_id` as it truly stands: a `Running` record whose
    /// process is gone is reaped first (when no job holds the VM) and is
    /// never reported as running. `None` when there is no such VM.
    async fn probe(&self, vm_id: &str) -> Option<VmRecord> {
        let (record, booted, lock) = self.entry(vm_id).await.ok()?;
        if record.state != VmState::Running || self.inner.hypervisor.alive(&booted).await {
            return Some(record);
        }
        match lock.try_lock_owned() {
            Ok(guard) => Some(self.reap(&record, &booted, guard).await),
            // A job is in flight on the dead VM: it fails on its own and reaps
            // on its way out. Meanwhile the VM is not running for anyone.
            Err(_) => Some(VmRecord {
                state: VmState::Crashed,
                ..record
            }),
        }
    }

    /// Release a dead VM's host resources per its retain policy and record
    /// it as crashed. The caller holds the VM's job lock so no job races the
    /// teardown.
    async fn reap(
        &self,
        record: &VmRecord,
        booted: &BootedVm,
        _job: OwnedMutexGuard<()>,
    ) -> VmRecord {
        let vm_id = &record.handle.vm_id;
        tracing::warn!(
            %vm_id, topic_id = %record.handle.topic_id, retain = ?record.retain,
            "topic vm process exited outside teardown; reaping"
        );
        match self.inner.hypervisor.teardown(booted, record.retain).await {
            Ok(true) => {}
            Ok(false) => tracing::error!(%vm_id, "reap: host did not confirm the release"),
            Err(e) => tracing::error!(%vm_id, "reap: {e}"),
        }
        let mut vms = self.inner.vms.write().await;
        match vms.get_mut(vm_id.as_str()) {
            Some(e) if e.record.state == VmState::Running => {
                e.record.state = VmState::Crashed;
                e.record.clone()
            }
            Some(e) => e.record.clone(),
            None => VmRecord {
                state: VmState::Crashed,
                ..record.clone()
            },
        }
    }

    /// The running, alive VM bound to `topic_id`, if any.
    async fn live_vm_for(&self, topic_id: &str) -> Option<VmRecord> {
        let candidates: Vec<String> = self
            .inner
            .vms
            .read()
            .await
            .values()
            .filter(|e| e.record.handle.topic_id == topic_id && e.record.state == VmState::Running)
            .map(|e| e.record.handle.vm_id.clone())
            .collect();
        for id in candidates {
            if let Some(rec) = self.probe(&id).await {
                if rec.state == VmState::Running {
                    return Some(rec);
                }
            }
        }
        None
    }
}

fn bind(record: &VmRecord, topic_id: &str, what: &str) -> Result<(), AgentError> {
    if record.handle.topic_id == topic_id.trim() {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::TopicMismatch,
            format!(
                "vm {} is bound to topic {:?}, {what} names {:?}",
                record.handle.vm_id, record.handle.topic_id, topic_id
            ),
        ))
    }
}

async fn require_bearer(State(state): State<AgentState>, req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match state.inner.auth.accepts(presented) {
        Ok(()) => next.run(req).await,
        Err(e) => AgentError::new(ErrorCode::Unauthorized, e.to_string()).into_response(),
    }
}

async fn health(State(state): State<AgentState>) -> Json<AgentHealth> {
    let (ready, reason) = match state.inner.hypervisor.ready() {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e.to_string()),
    };
    // Health is also where an idle host notices a VM that died in the meantime.
    state.sweep().await;
    Json(AgentHealth {
        api_version: API_VERSION,
        ready,
        reason,
        hypervisor: state.inner.hypervisor.name().to_owned(),
        vms: state.inner.vms.read().await.len(),
    })
}

async fn create_vm(
    State(state): State<AgentState>,
    Json(body): Json<CreateVmRequest>,
) -> Result<(StatusCode, Json<VmRecord>), AgentError> {
    let spec = body.spec;
    spec.validate()
        .map_err(|e| AgentError::new(ErrorCode::BadSpec, e.to_string()))?;
    state.inner.hypervisor.ready()?;
    // Serialise creates: the topic ↔ VM check and the insert must be one step.
    let _create = state.inner.create_lock.lock().await;
    // A dead VM is reaped here and does not count: the topic gets a fresh one.
    if let Some(existing) = state.live_vm_for(&spec.topic_id).await {
        return Err(AgentError::new(
            ErrorCode::AlreadyExists,
            format!(
                "topic {:?} already has vm {}",
                spec.topic_id, existing.handle.vm_id
            ),
        ));
    }
    let vm_id = state.mint_vm_id(&spec.topic_id);
    let booted = state.inner.hypervisor.boot(&vm_id, &spec).await?;
    if booted.topic_id != spec.topic_id || booted.vm_id != vm_id {
        return Err(AgentError::new(
            ErrorCode::Backend,
            "hypervisor booted a vm under another binding",
        ));
    }
    let record = VmRecord {
        handle: VmHandle {
            topic_id: spec.topic_id.clone(),
            vm_id: vm_id.clone(),
        },
        image_digest: booted.image_digest.clone(),
        vcpus: spec.template.vcpus,
        mem_mib: spec.template.mem_mib,
        sandbox: spec.sandbox.clone(),
        retain: spec.retain,
        state: VmState::Running,
    };
    tracing::info!(topic_id = %spec.topic_id, %vm_id, image = %booted.image_digest, "topic vm booted");
    state.inner.vms.write().await.insert(
        vm_id,
        VmEntry {
            record: record.clone(),
            booted,
            lock: Arc::new(Mutex::new(())),
        },
    );
    Ok((StatusCode::CREATED, Json(record)))
}

async fn attach(
    State(state): State<AgentState>,
    Path(topic_id): Path<String>,
) -> Result<Json<VmRecord>, AgentError> {
    state.live_vm_for(&topic_id).await.map(Json).ok_or_else(|| {
        AgentError::new(
            ErrorCode::NotFound,
            format!("no running vm for topic {topic_id:?}"),
        )
    })
}

fn not_running(vm_id: &str, state: VmState) -> AgentError {
    AgentError::new(
        ErrorCode::NotFound,
        format!("vm {vm_id} is {state:?}, not running"),
    )
}

async fn run_job(
    State(state): State<AgentState>,
    Path(vm_id): Path<String>,
    Json(body): Json<RunJobRequest>,
) -> Result<Json<RunJobResponse>, AgentError> {
    let (record, booted, lock) = state.entry(&vm_id).await?;
    if record.state != VmState::Running {
        return Err(not_running(&vm_id, record.state));
    }
    bind(&record, &body.topic_id, "the request")?;
    bind(&record, body.job.topic_id(), "the job")?;
    let guard = lock
        .try_lock_owned()
        .map_err(|_| AgentError::new(ErrorCode::Busy, format!("vm {vm_id} is running a job")))?;
    if !state.inner.hypervisor.alive(&booted).await {
        let reaped = state.reap(&record, &booted, guard).await;
        return Err(not_running(&vm_id, reaped.state));
    }
    let outcome = match state.inner.hypervisor.run_job(&booted, &body.job).await {
        Ok(outcome) => outcome,
        Err(e) => {
            // A guest that died under the job is reaped now, while we hold it.
            if !state.inner.hypervisor.alive(&booted).await {
                state.reap(&record, &booted, guard).await;
            }
            return Err(e.into());
        }
    };
    if !output_matches(&body.job, &outcome.output) {
        return Err(AgentError::new(
            ErrorCode::WrongOutput,
            "guest answered with another output shape",
        ));
    }
    // Evidence for one job never stamps another: the attestation and the
    // report must name this job's topic, submission, and artefact.
    bind_evidence(&body.job, &outcome.output, outcome.sister.as_ref())
        .map_err(|e| AgentError::new(ErrorCode::EvidenceMismatch, e.to_string()))?;
    if let Some(s) = &outcome.sister {
        tracing::info!(
            %vm_id, sister = %s.sister_vm_id, submission = %s.submission_digest,
            artifact = %s.artifact_digest, sandboxed = s.sandboxed,
            flops_used = ?s.flops_used, wall_ms = s.wall_ms, "sister guest run attested"
        );
    }
    let output = stamp_output(outcome.output, outcome.sister.as_ref());
    Ok(Json(RunJobResponse {
        topic_id: record.handle.topic_id,
        vm_id,
        output,
        sister: outcome.sister,
    }))
}

async fn teardown(
    State(state): State<AgentState>,
    Path(vm_id): Path<String>,
    Json(body): Json<TeardownRequest>,
) -> Result<Json<TeardownResponse>, AgentError> {
    let (record, booted, lock) = state.entry(&vm_id).await?;
    bind(&record, &body.topic_id, "the teardown")?;
    if record.state != VmState::Running {
        return Ok(Json(TeardownResponse {
            topic_id: record.handle.topic_id,
            vm_id,
            state: record.state,
            confirmed: true,
        }));
    }
    let _guard = lock
        .try_lock_owned()
        .map_err(|_| AgentError::new(ErrorCode::Busy, format!("vm {vm_id} is running a job")))?;
    let confirmed = state
        .inner
        .hypervisor
        .teardown(&booted, body.policy)
        .await?;
    let end = match body.policy {
        RetainPolicy::Destroy => VmState::Destroyed,
        RetainPolicy::Retain => VmState::Retained,
    };
    let state_now = if confirmed {
        let mut vms = state.inner.vms.write().await;
        match end {
            VmState::Destroyed => {
                vms.remove(&vm_id);
            }
            VmState::Retained | VmState::Running | VmState::Crashed => {
                if let Some(e) = vms.get_mut(&vm_id) {
                    e.record.state = end;
                }
            }
        }
        tracing::info!(%vm_id, topic_id = %record.handle.topic_id, ?end, "topic vm torn down");
        end
    } else {
        VmState::Running
    };
    Ok(Json(TeardownResponse {
        topic_id: record.handle.topic_id,
        vm_id,
        state: state_now,
        confirmed,
    }))
}

/// The agent router. Every route, health included, needs the bearer.
pub fn agent_router(state: AgentState) -> Router {
    Router::new()
        .route(paths::HEALTH, get(health))
        .route(paths::VMS, post(create_vm))
        .route("/v1/vms/by-topic/{topic_id}", get(attach))
        .route("/v1/vms/{vm_id}/jobs", post(run_job))
        .route("/v1/vms/{vm_id}", axum::routing::delete(teardown))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}
