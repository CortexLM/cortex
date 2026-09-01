//! Lium transport for a digest-pinned challenge eval image.
//!
//! Every live Cortex challenge scores on hardware the miner pays for, running
//! an image this repo only knows by digest. The lifecycle is the same for all
//! of them — boot the pinned digest, deliver a request over stdin, read one
//! marker line back, scrub, terminate with verification — so it lives here
//! once instead of being re-derived per challenge.
//!
//! Nothing here computes or interprets a score. The transport moves opaque
//! request bytes to the pod and returns the pod's stdout; deciding whether
//! that stdout is a verdict is the challenge's job, and every challenge binds
//! the document to its own pin before believing it.
//!
//! Errors are plain strings so a challenge can map them onto its own error
//! type without this crate depending on any challenge's types.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]

use async_trait::async_trait;
use prism_lium::{
    parse_ssh_target, resolve_private_key, ssh_exec, ssh_exec_allow_fail, ssh_exec_stdin,
    EvalJobBackend, LiumClient, SshTarget,
};
use prism_lium_types::InstanceSpec;

/// SSH attempts for each step.
const SSH_ATTEMPTS: u32 = 3;
/// Seconds between SSH attempts.
const SSH_RETRY_SECS: u64 = 10;
/// Timeout for the short setup / harvest commands.
const SSH_SHORT_TIMEOUT_SECS: u64 = 120;

/// What one challenge's eval image is called and what it prints.
///
/// The entrypoint and sidecar name are part of the image contract, documented
/// per challenge (for example `docs/RELEARN.md` § Eval image contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodProgram {
    /// Directory the request and metrics sidecar live in, on the pod.
    pub workdir: &'static str,
    /// Entrypoint the image exposes, minus the `--request` / `--out` flags.
    pub entrypoint: &'static str,
    /// Prefix the image prints before its one-line metrics document.
    pub metrics_marker: &'static str,
    /// Marker the image prints on a completed run.
    pub ok_marker: &'static str,
}

impl PodProgram {
    /// Command that stages the request under [`Self::workdir`] from stdin.
    ///
    /// Run inputs never reach the shell: a crafted digest interpolated into a
    /// command would be remote code execution on a pod the miner pays for.
    #[must_use]
    pub fn stage_cmd(&self) -> String {
        let dir = self.workdir;
        format!(
            "set -e; mkdir -p {dir}; cd {dir}; umask 077; cat > request.json; wc -c < request.json"
        )
    }

    /// Command that runs the image entrypoint and prints the metrics document.
    ///
    /// The log tail comes last so a consumer that truncates loses diagnostics
    /// rather than the metrics line.
    #[must_use]
    pub fn run_cmd(&self, timeout_secs: u64) -> String {
        let Self {
            workdir,
            entrypoint,
            metrics_marker,
            ok_marker,
        } = *self;
        format!(
            "set +e; cd {workdir} || exit 1; \
             timeout --kill-after=60 {timeout_secs} {entrypoint} \
               --request request.json --out metrics.json > run.log 2>&1; \
             rc=$?; \
             if [ -f metrics.json ]; then printf '{metrics_marker}'; cat metrics.json; printf '\\n'; fi; \
             if [ $rc -eq 0 ]; then echo {ok_marker}; fi; \
             tail -c 8192 run.log 2>/dev/null || true"
        )
    }

    /// Best-effort scrub so a private holdout does not outlive the run on a pod
    /// the miner is paying for. Termination is the real guarantee; this only
    /// narrows the window before it.
    #[must_use]
    pub fn scrub_cmd(&self) -> String {
        format!("rm -rf {} 2>/dev/null || true", self.workdir)
    }

    /// Pull the one-line metrics document out of the image's stdout.
    ///
    /// A bare JSON body is accepted too, so an image that writes only the
    /// sidecar still works.
    #[must_use]
    pub fn extract_document<'a>(&self, stdout: &'a str) -> Option<&'a str> {
        if let Some(line) = stdout.lines().find(|l| l.starts_with(self.metrics_marker)) {
            return Some(line[self.metrics_marker.len()..].trim());
        }
        let trimmed = stdout.trim();
        trimmed.starts_with('{').then_some(trimmed)
    }

    /// Whether the image reported a completed run.
    #[must_use]
    pub fn ran_to_completion(&self, stdout: &str) -> bool {
        stdout.lines().any(|l| l.trim_end() == self.ok_marker)
    }
}

/// One pod's lifecycle for one harvest.
///
/// Split from the challenge-side harvest so teardown and document verification
/// stay testable without a Lium account.
#[async_trait]
pub trait EvalPod: Send + Sync {
    /// Boot the digest-pinned image and return the instance id.
    async fn boot(&self, spec: &InstanceSpec) -> Result<String, String>;

    /// Deliver `request` bytes, run the image, return its stdout.
    async fn run(&self, instance_id: &str, request: &[u8]) -> Result<String, String>;

    /// Terminate. `Ok(true)` only when the provider confirms the pod is gone.
    async fn shutdown(&self, instance_id: &str) -> Result<bool, String>;
}

/// Lium-backed [`EvalPod`].
pub struct LiumEvalPod {
    client: LiumClient,
    /// Seconds the image gets to score one artifact.
    run_timeout_secs: u64,
    program: PodProgram,
}

impl LiumEvalPod {
    /// Wrap a configured Lium client around one challenge's image contract.
    #[must_use]
    pub fn new(client: LiumClient, run_timeout_secs: u64, program: PodProgram) -> Self {
        Self {
            client,
            run_timeout_secs: run_timeout_secs.max(60),
            program,
        }
    }

    /// The image contract this pod runs.
    #[must_use]
    pub const fn program(&self) -> PodProgram {
        self.program
    }

    async fn target(&self, instance_id: &str) -> Result<SshTarget, String> {
        let raw = self
            .client
            .get_pod_raw(instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let cmd = raw
            .get("ssh_connect_cmd")
            .and_then(|x| x.as_str())
            .unwrap_or_default();
        parse_ssh_target(cmd, &raw).ok_or_else(|| format!("no ssh target for pod {instance_id}"))
    }
}

#[async_trait]
impl EvalPod for LiumEvalPod {
    async fn boot(&self, spec: &InstanceSpec) -> Result<String, String> {
        // `provision` applies the cost guardrails and cleans up on failure.
        let inst = self.client.provision(spec).await.map_err(|e| e.to_string())?;
        self.client
            .wait_until_running(&inst.id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(inst.id)
    }

    async fn run(&self, instance_id: &str, request: &[u8]) -> Result<String, String> {
        let key = resolve_private_key(None).map_err(|e| e.to_string())?;
        let target = self.target(instance_id).await?;

        ssh_exec_stdin(
            &target,
            &key,
            &self.program.stage_cmd(),
            request,
            SSH_ATTEMPTS,
            SSH_RETRY_SECS,
            SSH_SHORT_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| format!("stage request: {e}"))?;

        // `allow_fail`: a non-zero image exit still has to be harvested, since
        // the log tail is the only diagnosis the operator gets.
        let out = ssh_exec_allow_fail(
            &target,
            &key,
            &self.program.run_cmd(self.run_timeout_secs),
            1,
            SSH_RETRY_SECS,
            self.run_timeout_secs.saturating_add(SSH_SHORT_TIMEOUT_SECS),
        )
        .await
        .map_err(|e| format!("run eval image: {e}"))?;

        let _ = ssh_exec(
            &target,
            &key,
            &self.program.scrub_cmd(),
            1,
            SSH_RETRY_SECS,
            SSH_SHORT_TIMEOUT_SECS,
        )
        .await;

        Ok(out.stdout)
    }

    async fn shutdown(&self, instance_id: &str) -> Result<bool, String> {
        self.client
            .terminate(instance_id)
            .await
            .map_err(|e| e.to_string())?;
        self.client
            .verify_terminated(instance_id)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: PodProgram = PodProgram {
        workdir: "/tmp/demo_eval",
        entrypoint: "demo-eval score",
        metrics_marker: "DEMO_METRICS=",
        ok_marker: "DEMO_EVAL_OK",
    };

    #[test]
    fn the_run_command_harvests_the_sidecar_and_the_marker() {
        let cmd = PROGRAM.run_cmd(600);
        assert!(cmd.contains(PROGRAM.metrics_marker));
        assert!(cmd.contains(PROGRAM.ok_marker));
        assert!(cmd.contains("cat metrics.json"));
        assert!(cmd.contains("timeout --kill-after=60 600"));
        let doc = cmd.find(PROGRAM.metrics_marker).unwrap_or(0);
        let tail = cmd.find("tail -c").unwrap_or(0);
        assert!(doc < tail, "{cmd}");
    }

    #[test]
    fn the_request_never_reaches_the_shell() {
        let cmd = PROGRAM.run_cmd(600);
        assert!(cmd.contains("--request request.json"));
        assert!(!cmd.contains("artifact_digest"));
        assert!(PROGRAM.stage_cmd().contains("cat > request.json"));
        assert!(PROGRAM.stage_cmd().contains("umask 077"));
    }

    #[test]
    fn the_workdir_is_scrubbed_after_the_run() {
        assert!(PROGRAM.scrub_cmd().contains(PROGRAM.workdir));
        assert!(PROGRAM.scrub_cmd().starts_with("rm -rf "));
    }

    #[test]
    fn documents_come_from_the_marker_line_or_a_bare_body() {
        let with_marker = "boot ok\nDEMO_METRICS={\"a\":1}\nDEMO_EVAL_OK\n";
        assert_eq!(PROGRAM.extract_document(with_marker), Some("{\"a\":1}"));
        assert!(PROGRAM.ran_to_completion(with_marker));
        assert_eq!(PROGRAM.extract_document(" {\"a\":1} "), Some("{\"a\":1}"));
        assert_eq!(PROGRAM.extract_document("segfault"), None);
        assert!(!PROGRAM.ran_to_completion("segfault"));
    }

    #[test]
    fn run_timeout_has_a_floor() {
        let pod = LiumEvalPod::new(
            LiumClient::new("unused-in-this-test").expect("client"),
            1,
            PROGRAM,
        );
        assert_eq!(pod.run_timeout_secs, 60);
        assert_eq!(pod.program(), PROGRAM);
    }
}
