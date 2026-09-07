use std::{
    fmt::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use chain_live::FinalizedSnapshot;
use proof_atlas_worker::{AtlasAgent, AtlasChain, AtlasError, AtlasJob};
use proof_rounds::{FinalizedRoundSource, RoundError, RoundPublication, RoundPublisher};
use proof_runtime::RuntimeOperations;
use proof_worker::{
    HeadlessConfig, HeadlessInvocation, HeadlessProcess, InferenceBudget, KernelLimits,
};
use serde_json::{json, Value};
use tokio::{
    sync::{watch, Mutex},
    task::JoinHandle,
};

use super::rounds;

pub const IMAGE: &str = "sha256:0104307df448338d8475c7cf8152e5e0655e211fd1c04b2bdc94e6758a7e7293";
pub const PROMPT: &str = "Synthetic local Atlas IPC integration test. Read frozen evidence and history, inspect the retained artifact, and submit the fixture decision through private controller operations. No provider operations, rentals, external publication, or scientific claims.";

pub struct Source(pub rounds::Source);
impl AtlasChain for Source {
    fn finalized_height(&self) -> Result<u64, AtlasError> {
        Ok(360)
    }
}
impl FinalizedRoundSource for Source {
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError> {
        if block != 360 {
            return Err(RoundError::Invalid);
        }
        self.0.boundary(block)
    }
}
impl gateway_proof::FinalizedSource for Source {
    fn snapshot(&self, block: u64) -> Result<FinalizedSnapshot, gateway_proof::Error> {
        self.boundary(block)
            .map_err(|_| gateway_proof::Error::Invalid)
    }
}

pub struct Server {
    pub url: String,
    task: JoinHandle<()>,
}
impl Server {
    pub async fn start(app: Router) -> Self {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(socket, app).await.unwrap();
        });
        Self { url, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct Model {
    pub requests: Mutex<Vec<Value>>,
}
impl Model {
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/v1/responses", post(inference))
            .with_state(self.clone())
    }
}

async fn inference(
    State(model): State<Arc<Model>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Result<([(String, String); 1], String), StatusCode> {
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some("Bearer synthetic-atlas-key")
        || request["model"] != "atlas-local-faux"
        || request["stream"] != true
        || request["store"] != false
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut requests = model.requests.lock().await;
    let index = requests.len();
    requests.push(request.clone());
    let item = match index {
        0 => {
            json!({"type":"function_call", "id":"fc_atlas", "call_id":"call_atlas", "name":"ipython",
            "status":"completed", "arguments":serde_json::to_string(&json!({"code":
                include_str!("submit_decision.py").replace("__SCORING_VERSION__",
                    &proof_autonomy::ATLAS_SCORING_VERSION.to_string())
            })).unwrap()})
        }
        1 if request["input"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["type"] == "function_call_output"
                    && item["output"]
                        .to_string()
                        .contains("ATLAS_KERNEL_DECISION_COMMITTED")
            })
        }) =>
        {
            json!({"type":"message", "id":"msg_atlas", "role":"assistant", "status":"completed",
            "content":[{"type":"output_text", "text":"Synthetic private decision completed.", "annotations":[]}]})
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let events = [
        json!({"type":"response.created","response":{"id":"resp_atlas"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":"resp_atlas","status":"completed","output":[item],
            "usage":{"input_tokens":100,"output_tokens":100,"total_tokens":200}}}),
    ];
    let mut stream = String::new();
    for event in &events {
        write!(
            stream,
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap()
        )
        .unwrap();
    }
    Ok((
        [("content-type".into(), "text/event-stream".into())],
        format!("{stream}data: [DONE]\n\n"),
    ))
}

pub struct Agent {
    process: HeadlessProcess,
    pub jobs: Mutex<Vec<AtlasJob>>,
    pub failures: Mutex<Vec<String>>,
}
impl Agent {
    pub fn new(root: &Path, model_url: &str) -> Arc<Self> {
        let fork = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../agents/atlas")
            .canonicalize()
            .unwrap();
        let config = root.join("model.json");
        let key = root.join("model-key");
        private(&key, b"synthetic-atlas-key\n");
        private(&config, &serde_json::to_vec(&json!({
            "schema_version":1, "provider":"openai", "model":"atlas-local-faux", "api":"openai-responses",
            "baseUrl":format!("{model_url}/v1"), "allowLoopbackHttp":true, "apiKeyFile":key,
            "reasoning":false, "contextWindow":16384, "maxTokens":4096,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
        })).unwrap());
        let process = HeadlessProcess::new(HeadlessConfig {
            node: "/usr/bin/node".into(),
            loader: fork.join("node_modules/tsx/dist/loader.mjs"),
            entrypoint: fork.join("packages/coding-agent/src/cortex/headless-cli.ts"),
            tsconfig: fork.join("tsconfig.json"),
            private_root: root.into(),
            model_config_file: config,
            kernel_python: "/tmp/cortex-atlas-upstream-python/bin/python".into(),
            runtime_pythonpath: fork.join("prime-agent-runtime/src"),
            // Exercise the full runtime as PID 1 of a private namespace when
            // the host supports unprivileged `unshare`.
            pid_namespace: Some(PathBuf::from("/usr/bin/unshare")).filter(|p| p.exists()),
            kernel: KernelLimits {
                image: IMAGE.into(),
                memory_mb: 512,
                workspace_mb: 64,
                cpus: 1,
                pids: 64,
                seconds: 90,
            },
            budget: InferenceBudget {
                max_depth: 1,
                max_children: 1,
                max_concurrent_calls: 1,
                max_calls: 4,
                max_reserved_tokens: 100_000,
                max_reserved_micro_usd: 1,
                timeout_ms: 90_000,
            },
        })
        .unwrap();
        Arc::new(Self {
            process,
            jobs: Mutex::new(vec![]),
            failures: Mutex::new(vec![]),
        })
    }
}
#[async_trait]
impl AtlasAgent for Agent {
    fn binding(&self) -> Result<String, AtlasError> {
        self.process
            .binding(PROMPT)
            .map_err(|_| AtlasError::Invalid)
    }
    fn maximum_seconds(&self) -> u32 {
        self.process.maximum_seconds()
    }
    async fn run(
        &self,
        job: &AtlasJob,
        operations: Arc<dyn RuntimeOperations>,
        stop: watch::Receiver<bool>,
    ) -> Result<(), AtlasError> {
        self.jobs.lock().await.push(job.clone());
        let invocation = HeadlessInvocation {
            runtime_id: job.run.id,
            deadline_ms: job.run.deadline_ms,
            resume: job.resume,
            scope: job.scope.clone(),
            prompt: PROMPT.into(),
        };
        match self.process.run(&invocation, operations, stop).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failures.lock().await.push(error.to_string());
                Err(AtlasError::Unavailable)
            }
        }
    }
}

pub struct LostAcknowledgement {
    pub client: proof_publication::Client,
    pub documents: Mutex<Vec<RoundPublication>>,
    pub lose_once: AtomicBool,
}
#[async_trait]
impl RoundPublisher for LostAcknowledgement {
    async fn publish(&self, document: &RoundPublication) -> Result<String, RoundError> {
        self.documents.lock().await.push(document.clone());
        let digest = self
            .client
            .publish(document)
            .await
            .map_err(|_| RoundError::Publication)?;
        if self.lose_once.swap(false, Ordering::SeqCst) {
            return Err(RoundError::Publication);
        }
        Ok(digest)
    }
}

fn private(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn checkpoint(root: &Path, job: &AtlasJob) -> Value {
    let directory = root.join(job.run.id.to_string());
    assert!(directory.join("state/headless.json").is_file());
    assert!(directory.join("state/tree.json").is_file());
    let budget: Value =
        serde_json::from_slice(&std::fs::read(directory.join("state/budget.json")).unwrap())
            .unwrap();
    let mut kernels = 0;
    for entry in std::fs::read_dir(directory.join("sandbox")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let kernel: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(kernel["deadline_ms"], job.run.deadline_ms);
        assert_eq!(kernel["image"], IMAGE);
        let name = kernel["name"].as_str().unwrap();
        let output = std::process::Command::new("/usr/bin/docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("label=cortex.kernel={name}"),
                "--format={{.ID}}",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "kernel container survived completed run"
        );
        kernels += 1;
    }
    assert_eq!(kernels, 1);
    budget
}
