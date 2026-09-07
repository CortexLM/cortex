use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use proof_autonomy::commitment;
use proof_research::ResearchStore;
use proof_runtime::{ExperimentExecutor, ExperimentOperations, RuntimeScope};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{ExperimentAgent, RuntimeJob, WorkerError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelLimits {
    pub image: String,
    pub memory_mb: u32,
    pub workspace_mb: u32,
    pub cpus: u32,
    pub pids: u32,
    pub seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InferenceBudget {
    pub max_depth: u32,
    pub max_children: u32,
    pub max_concurrent_calls: u32,
    pub max_calls: u32,
    pub max_reserved_tokens: u64,
    pub max_reserved_micro_usd: u64,
    pub timeout_ms: u32,
}

/// Nonsecret, operator-owned configuration. The launcher validates the private
/// model/key files itself and never inherits credentials from the environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadlessConfig {
    pub node: PathBuf,
    pub loader: PathBuf,
    pub entrypoint: PathBuf,
    pub tsconfig: PathBuf,
    pub private_root: PathBuf,
    pub model_config_file: PathBuf,
    pub kernel_python: PathBuf,
    pub runtime_pythonpath: PathBuf,
    /// Absolute `unshare` executable. When set, the launcher runs as PID 1 of a
    /// private PID namespace so a `setsid` escape from the process group still
    /// dies with the launcher. This is not a mount/network/user sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_namespace: Option<PathBuf>,
    pub kernel: KernelLimits,
    pub budget: InferenceBudget,
}

#[derive(Clone)]
pub struct HeadlessProcess {
    pub(crate) config: HeadlessConfig,
}

#[derive(Debug, Clone)]
pub struct HeadlessInvocation {
    pub runtime_id: Uuid,
    pub deadline_ms: i64,
    pub resume: bool,
    pub scope: RuntimeScope,
    pub prompt: String,
}

impl HeadlessProcess {
    /// # Errors
    /// Unsafe paths or invalid bounds. Provider configuration is checked again
    /// in the isolated launcher; this does not attest installed source code.
    pub fn new(config: HeadlessConfig) -> Result<Self, WorkerError> {
        let mut paths = vec![
            &config.node,
            &config.loader,
            &config.entrypoint,
            &config.tsconfig,
            &config.model_config_file,
            &config.kernel_python,
            &config.runtime_pythonpath,
        ];
        paths.extend(config.pid_namespace.as_ref());
        for path in paths {
            if !path.is_absolute() || !path.exists() {
                return Err(WorkerError::Invalid);
            }
        }
        private_directory(&config.private_root)?;
        if config.budget.timeout_ms == 0
            || config.budget.timeout_ms > 86_400_000
            || config.budget.max_calls == 0
            || config.kernel.seconds == 0
        {
            return Err(WorkerError::Invalid);
        }
        Ok(Self { config })
    }

    /// # Errors
    /// Invalid serializable runtime configuration.
    pub fn binding(&self, prompt: &str) -> Result<String, WorkerError> {
        Ok(commitment(&(&self.config, prompt))?)
    }

    #[must_use]
    pub fn maximum_seconds(&self) -> u32 {
        self.config.budget.timeout_ms.div_ceil(1000)
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<(), WorkerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || std::fs::canonicalize(path)? != path
    {
        return Err(WorkerError::Invalid);
    }
    Ok(())
}

pub(crate) fn create_private(path: &Path) -> Result<(), WorkerError> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    private_directory(path)
}

pub struct HeadlessExperiment {
    pub process: HeadlessProcess,
    pub pool: PgPool,
    pub research: ResearchStore,
    pub executor: Arc<dyn ExperimentExecutor>,
    pub prompt: String,
}

#[async_trait]
impl ExperimentAgent for HeadlessExperiment {
    fn binding(&self) -> Result<String, WorkerError> {
        self.process.binding(&self.prompt)
    }
    fn maximum_seconds(&self) -> u32 {
        self.process.maximum_seconds()
    }
    async fn run(&self, job: &RuntimeJob, stop: watch::Receiver<bool>) -> Result<(), WorkerError> {
        let operations = ExperimentOperations::bind(
            self.pool.clone(),
            self.research.clone(),
            job.lease,
            job.resource.resource_id.clone(),
            self.executor.clone(),
        )
        .await?;
        let invocation = HeadlessInvocation {
            runtime_id: job.run.id,
            deadline_ms: job.run.deadline_ms,
            resume: job.resume,
            scope: operations.scope().clone(),
            prompt: self.prompt.clone(),
        };
        self.process
            .run(&invocation, Arc::new(operations), stop)
            .await
    }
}
