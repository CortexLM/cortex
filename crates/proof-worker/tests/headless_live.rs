#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Explicit opt-in only. Sends a synthetic prompt to the configured provider.
use async_trait::async_trait;
use proof_runtime::{RuntimeCall, RuntimeError, RuntimeOperations, RuntimeScope};
use proof_worker::*;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex};
use uuid::Uuid;

struct Operations(Mutex<Vec<String>>);
#[async_trait]
impl RuntimeOperations for Operations {
    async fn call(&self, call: RuntimeCall) -> Result<Value, RuntimeError> {
        if call.operation != "report"
            || call.arguments != json!({"text": "astra-private-roundtrip"})
        {
            return Err(RuntimeError::Scope);
        }
        self.0.lock().await.push(call.operation);
        Ok(json!({"accepted": true}))
    }
}

#[tokio::test]
#[ignore = "requires explicit authorized model configuration and an installed kernel image"]
async fn authorized_model_calls_controller_through_real_isolated_kernel() {
    let model = std::env::var_os("CORTEX_TEST_HEADLESS_MODEL_CONFIG")
        .expect("CORTEX_TEST_HEADLESS_MODEL_CONFIG must explicitly authorize the test");
    let image = std::env::var("CORTEX_TEST_KERNEL_IMAGE").expect("pinned local image");
    let python =
        std::env::var_os("PRIME_AGENT_KERNEL_PYTHON").expect("isolated Python prerequisite");
    let fork = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../agents/atlas")
        .canonicalize()
        .unwrap();
    let root = tempfile::Builder::new()
        .prefix("cpx-live-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let process = HeadlessProcess::new(HeadlessConfig {
        node: "/usr/bin/node".into(),
        loader: fork.join("node_modules/tsx/dist/loader.mjs"),
        entrypoint: fork.join("packages/coding-agent/src/cortex/headless-cli.ts"),
        tsconfig: fork.join("tsconfig.json"),
        private_root: root.path().into(),
        model_config_file: model.into(),
        kernel_python: python.into(),
        runtime_pythonpath: fork.join("prime-agent-runtime/src"),
        pid_namespace: None,
        kernel: KernelLimits {
            image,
            memory_mb: 512,
            workspace_mb: 64,
            cpus: 1,
            pids: 64,
            seconds: 120,
        },
        budget: InferenceBudget {
            max_depth: 1,
            max_children: 1,
            max_concurrent_calls: 1,
            max_calls: 4,
            max_reserved_tokens: 100_000,
            max_reserved_micro_usd: 10_000_000,
            timeout_ms: 120_000,
        },
    })
    .unwrap();
    let invocation = HeadlessInvocation {
        runtime_id: Uuid::new_v4(), deadline_ms: i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap() + 120_000,
        resume: false, scope: RuntimeScope { role: "experiment".into(), id: Uuid::new_v4().to_string(), commitment: "b".repeat(64) },
        prompt: "This is a synthetic IPC test, not scientific research. Use the ipython tool exactly once to execute:\nfrom rlm import host_request\nresult = await host_request('cortex.call', {'operation': 'report', 'arguments': {'text': 'astra-private-roundtrip'}})\nassert result['accepted'] is True\nprint('ok')\nThen finish. Do not spawn children, inspect files, access the network, or perform any other controller operation.".into(),
    };
    let operations = Arc::new(Operations(Mutex::new(vec![])));
    let (_stop, signal) = watch::channel(false);
    process
        .run(&invocation, operations.clone(), signal)
        .await
        .unwrap();
    assert_eq!(*operations.0.lock().await, vec!["report"]);
}
