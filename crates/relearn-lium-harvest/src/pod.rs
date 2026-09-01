//! Real Lium transport: boot the pinned image, deliver the request over SSH,
//! read the metrics document, terminate.
//!
//! Everything provider-specific lives here so [`crate::LiumHarvest`] can be
//! tested without a Lium account. No secret is logged: the API key stays
//! inside [`LiumClient`] and the request body is never printed.

use async_trait::async_trait;
use prism_lium::{
    parse_ssh_target, resolve_private_key, ssh_exec, ssh_exec_allow_fail, ssh_exec_stdin,
    EvalJobBackend, LiumClient, SshTarget,
};
use prism_lium_types::InstanceSpec;
use relearn_eval::EvalError;

use crate::{EvalPod, HarvestRequest, METRICS_MARKER, OK_MARKER, POD_WORKDIR};

/// SSH attempts for each step.
const SSH_ATTEMPTS: u32 = 3;
/// Seconds between SSH attempts.
const SSH_RETRY_SECS: u64 = 10;
/// Timeout for the short setup / harvest commands.
const SSH_SHORT_TIMEOUT_SECS: u64 = 120;

/// Command that stages one file under [`POD_WORKDIR`] from stdin.
///
/// `name` is a fixed control-plane constant, never miner input.
fn stage_cmd(name: &str) -> String {
    format!(
        "set -e; mkdir -p {POD_WORKDIR}; cd {POD_WORKDIR}; \
         umask 077; cat > {name}; wc -c < {name}"
    )
}

/// Env file the pod sources before the entrypoint.
const ENV_FILE: &str = "teacher.env";

/// Request file the image parses.
const REQUEST_FILE: &str = "request.json";

/// Command that runs the image entrypoint and prints the metrics document.
///
/// The entrypoint and the sidecar name are the image contract; see
/// `docs/RELEARN.md` § Eval image contract. Nothing miner-controlled and no
/// secret is interpolated here — run inputs travel in `request.json` and the
/// teacher config in `teacher.env`, both delivered over stdin. `set -a` is
/// what puts the env file's values in the image's environment; a Lium
/// `InstanceSpec` has no env field, so the pod inherits nothing otherwise.
fn run_cmd(timeout_secs: u64) -> String {
    format!(
        "set +e; cd {POD_WORKDIR} || exit 1; \
         set -a; [ -f {ENV_FILE} ] && . ./{ENV_FILE}; set +a; \
         timeout --kill-after=60 {timeout_secs} relearn-eval score \
           --request {REQUEST_FILE} --out metrics.json > run.log 2>&1; \
         rc=$?; \
         if [ -f metrics.json ]; then printf '{METRICS_MARKER}'; cat metrics.json; printf '\\n'; fi; \
         if [ $rc -eq 0 ]; then echo {OK_MARKER}; else echo \"exit=$rc\"; fi; \
         tail -c 8192 run.log 2>/dev/null || true"
    )
}

/// Best-effort scrub so the private split does not outlive the run on a pod
/// the miner is paying for. Termination is the real guarantee; this narrows
/// the window before it.
fn scrub_cmd() -> String {
    format!("rm -rf {POD_WORKDIR} 2>/dev/null || true")
}

/// Lium-backed [`EvalPod`].
pub struct LiumEvalPod {
    client: LiumClient,
    /// Seconds the image gets to score one artifact.
    run_timeout_secs: u64,
}

impl LiumEvalPod {
    /// Wrap a configured Lium client.
    #[must_use]
    pub fn new(client: LiumClient, run_timeout_secs: u64) -> Self {
        Self {
            client,
            run_timeout_secs: run_timeout_secs.max(60),
        }
    }

    async fn target(&self, instance_id: &str) -> Result<SshTarget, EvalError> {
        let raw = self
            .client
            .get_pod_raw(instance_id)
            .await
            .map_err(|e| EvalError::Backend(e.to_string()))?;
        let cmd = raw
            .get("ssh_connect_cmd")
            .and_then(|x| x.as_str())
            .unwrap_or_default();
        parse_ssh_target(cmd, &raw)
            .ok_or_else(|| EvalError::Backend(format!("no ssh target for pod {instance_id}")))
    }
}

#[async_trait]
impl EvalPod for LiumEvalPod {
    async fn boot(&self, spec: &InstanceSpec) -> Result<String, EvalError> {
        // `provision` applies the cost guardrails and cleans up on failure.
        let inst = self
            .client
            .provision(spec)
            .await
            .map_err(|e| EvalError::Backend(e.to_string()))?;
        self.client
            .wait_until_running(&inst.id)
            .await
            .map_err(|e| EvalError::Backend(e.to_string()))?;
        Ok(inst.id)
    }

    async fn run(
        &self,
        instance_id: &str,
        request: &HarvestRequest,
        env_file: &str,
    ) -> Result<String, EvalError> {
        let key = resolve_private_key(None).map_err(|e| EvalError::Backend(e.to_string()))?;
        let target = self.target(instance_id).await?;
        let body = serde_json::to_vec(request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        ssh_exec_stdin(
            &target,
            &key,
            &stage_cmd(REQUEST_FILE),
            &body,
            SSH_ATTEMPTS,
            SSH_RETRY_SECS,
            SSH_SHORT_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| EvalError::Backend(format!("stage request: {e}")))?;

        // Over stdin, not the command line: the teacher key would otherwise sit
        // in the pod's process table, on hardware the miner pays for.
        ssh_exec_stdin(
            &target,
            &key,
            &stage_cmd(ENV_FILE),
            env_file.as_bytes(),
            SSH_ATTEMPTS,
            SSH_RETRY_SECS,
            SSH_SHORT_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| EvalError::Backend(format!("stage teacher env: {e}")))?;

        // `allow_fail`: a non-zero image exit still has to be harvested, since
        // the log tail is the only diagnosis the operator gets.
        let out = ssh_exec_allow_fail(
            &target,
            &key,
            &run_cmd(self.run_timeout_secs),
            1,
            SSH_RETRY_SECS,
            self.run_timeout_secs.saturating_add(SSH_SHORT_TIMEOUT_SECS),
        )
        .await
        .map_err(|e| EvalError::Backend(format!("run eval image: {e}")))?;

        let _ = ssh_exec(
            &target,
            &key,
            &scrub_cmd(),
            1,
            SSH_RETRY_SECS,
            SSH_SHORT_TIMEOUT_SECS,
        )
        .await;

        Ok(out.stdout)
    }

    async fn shutdown(&self, instance_id: &str) -> Result<bool, EvalError> {
        self.client
            .terminate(instance_id)
            .await
            .map_err(|e| EvalError::Backend(e.to_string()))?;
        self.client
            .verify_terminated(instance_id)
            .await
            .map_err(|e| EvalError::Backend(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_command_harvests_the_sidecar_and_the_marker() {
        let cmd = run_cmd(600);
        assert!(cmd.contains(METRICS_MARKER));
        assert!(cmd.contains(OK_MARKER));
        assert!(cmd.contains("cat metrics.json"));
        assert!(cmd.contains("timeout --kill-after=60 600"));
        // The log tail must come after the document, so a truncating consumer
        // loses diagnostics rather than the metrics line.
        let doc = cmd.find(METRICS_MARKER).unwrap_or(0);
        let tail = cmd.find("tail -c").unwrap_or(0);
        assert!(doc < tail, "{cmd}");
    }

    #[test]
    fn the_request_never_reaches_the_shell() {
        // Run inputs travel in request.json over stdin. If they were ever
        // interpolated into the command, a crafted digest would be shell
        // injection on the pod.
        let cmd = run_cmd(600);
        assert!(cmd.contains("--request request.json"));
        assert!(!cmd.contains("artifact_digest"));
        assert!(stage_cmd(REQUEST_FILE).contains("cat > request.json"));
        assert!(stage_cmd(REQUEST_FILE).contains("umask 077"));
    }

    #[test]
    fn the_pod_env_is_sourced_from_a_file_not_the_command_line() {
        // A Lium InstanceSpec has no env field, so `set -a` + the env file is
        // the only way the image sees RELEARN_TEACHER_*. Putting values in the
        // command would publish the teacher key to the pod's process table.
        let cmd = run_cmd(600);
        assert!(cmd.contains("set -a; [ -f teacher.env ] && . ./teacher.env; set +a"));
        let sourced = cmd.find("teacher.env").unwrap_or(usize::MAX);
        let scored = cmd.find("relearn-eval score").unwrap_or(0);
        assert!(
            sourced < scored,
            "env must be sourced before the run: {cmd}"
        );
        assert!(!cmd.contains("RELEARN_TEACHER"), "no values in the command");
        assert!(stage_cmd(ENV_FILE).contains("cat > teacher.env"));
        assert!(stage_cmd(ENV_FILE).contains("umask 077"));
    }

    #[test]
    fn a_non_zero_exit_is_reported_in_stdout() {
        // Without this the operator sees an empty tail and no exit code for a
        // pod that booted, ran, and printed no marker.
        assert!(run_cmd(600).contains("echo \"exit=$rc\""));
    }

    #[test]
    fn the_workdir_is_scrubbed_after_the_run() {
        assert!(scrub_cmd().contains(POD_WORKDIR));
        assert!(scrub_cmd().starts_with("rm -rf "));
    }

    #[test]
    fn run_timeout_has_a_floor() {
        let pod = LiumEvalPod::new(LiumClient::new("unused-in-this-test").expect("client"), 1);
        assert_eq!(pod.run_timeout_secs, 60);
    }
}
