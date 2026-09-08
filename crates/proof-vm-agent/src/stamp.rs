//! Host authority over paid outputs.
//!
//! The RLM guest authors the report, but two fields are **facts about the
//! host**, so the agent overwrites them from what it booted and observed:
//!
//! - `sandboxed` is `true` only when the host booted a sister Firecracker
//!   guest for this job and the run happened inside it. An RLM that claims
//!   `sandboxed: true` without a sister is corrected to `false`, and the
//!   control plane then refuses the report for a `firecracker_required`
//!   topic (`ReportError::NotSandboxed`).
//! - `flops_used` is the sister guest's measurement when a sister ran. A
//!   sister that measured nothing yields `None`, which the control plane
//!   refuses against a budget (`ReportError::FlopsMissing`, 503, no row) —
//!   the RLM's own figure is never substituted for a run it did not perform.
//!
//! Inspection and rule proposals run no miner code and are passed through.

use proof_rlm::{CustomRunReport, VmJob, VmJobOutput};
use proof_vm_proto::SisterAttestation;

/// Whether `output` is the shape `job` asks for.
#[must_use]
pub fn output_matches(job: &VmJob, output: &VmJobOutput) -> bool {
    matches!(
        (job, output),
        (VmJob::ProposeRules { .. }, VmJobOutput::Rules(_))
            | (VmJob::Baseline { .. }, VmJobOutput::Baseline(_))
            | (VmJob::Inspect { .. }, VmJobOutput::Inspected(_))
            | (VmJob::Evaluate { .. }, VmJobOutput::Evaluated(_))
            | (VmJob::Archive { .. }, VmJobOutput::Archived)
    )
}

fn stamp_report(report: &mut CustomRunReport, sister: Option<&SisterAttestation>) {
    match sister {
        Some(s) => {
            report.sandboxed = s.sandboxed;
            report.flops_used = s.flops_used;
        }
        None => report.sandboxed = false,
    }
}

/// Apply the host's view to a paid output. Non-paid outputs are unchanged.
#[must_use]
pub fn stamp_output(mut output: VmJobOutput, sister: Option<&SisterAttestation>) -> VmJobOutput {
    match &mut output {
        VmJobOutput::Baseline(report) => stamp_report(report, sister),
        VmJobOutput::Evaluated(run) => stamp_report(&mut run.report, sister),
        VmJobOutput::Rules(_) | VmJobOutput::Inspected(_) | VmJobOutput::Archived => {}
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_rlm::fixtures::{report_for, request, rules};
    use proof_rlm::RunOutcome;

    fn sister(sandboxed: bool, flops: Option<u64>) -> SisterAttestation {
        SisterAttestation {
            sister_vm_id: "topic-a-0001-s1".into(),
            image_digest: format!("sha256:{}", "dd".repeat(32)),
            sandboxed,
            network: "none".into(),
            flops_used: flops,
            wall_ms: 10,
            exit_code: Some(0),
        }
    }

    #[test]
    fn the_rlm_cannot_claim_a_sandbox_the_host_did_not_boot() {
        let req = request();
        let mut claimed = report_for(&req, 0.9);
        claimed.sandboxed = true;
        claimed.flops_used = Some(5);
        let out = stamp_output(
            VmJobOutput::Evaluated(RunOutcome {
                report: claimed.clone(),
                logs: vec![],
            }),
            None,
        );
        let VmJobOutput::Evaluated(run) = out else {
            panic!("shape");
        };
        assert!(!run.report.sandboxed, "no sister, no sandbox");
        assert_eq!(
            run.report.flops_used,
            Some(5),
            "the RLM ran it itself; its own measurement stands"
        );
        assert!(
            run.report.verify(&req).is_err(),
            "firecracker_required topic refuses the corrected report"
        );
    }

    #[test]
    fn a_sister_run_stamps_sandboxed_and_the_guest_measurement() {
        let req = request();
        let mut lied = report_for(&req, 0.9);
        lied.sandboxed = false;
        lied.flops_used = Some(1);
        let out = stamp_output(VmJobOutput::Baseline(lied), Some(&sister(true, Some(42))));
        let VmJobOutput::Baseline(report) = out else {
            panic!("shape");
        };
        assert!(report.sandboxed);
        assert_eq!(report.flops_used, Some(42), "host-relayed guest figure");
        let none = stamp_output(
            VmJobOutput::Baseline(report_for(&req, 0.9)),
            Some(&sister(true, None)),
        );
        let VmJobOutput::Baseline(report) = none else {
            panic!("shape");
        };
        assert_eq!(
            report.flops_used, None,
            "a sister that measured nothing is not the RLM's number"
        );
        assert!(matches!(
            report.verify(&req),
            Err(proof_rlm::ReportError::FlopsMissing { .. })
        ));
    }

    #[test]
    fn non_paid_outputs_pass_through_and_shapes_are_checked() {
        let rules = rules();
        let out = stamp_output(
            VmJobOutput::Rules(rules.rules.clone()),
            Some(&sister(true, Some(1))),
        );
        assert_eq!(out, VmJobOutput::Rules(rules.rules.clone()));
        let req = request();
        let inspect = VmJob::Inspect {
            request: req.clone(),
            rules: rules.clone(),
        };
        assert!(output_matches(
            &inspect,
            &VmJobOutput::Inspected(proof_rlm::InspectOutcome {
                checklist: proof_rlm::fixtures::green(&rules, &req.submission_digest),
                artifact: vec![],
            })
        ));
        assert!(!output_matches(&inspect, &VmJobOutput::Archived));
        assert!(output_matches(
            &VmJob::Archive {
                topic_id: req.topic_id.clone()
            },
            &VmJobOutput::Archived
        ));
        assert!(!output_matches(
            &VmJob::Baseline {
                request: req.clone()
            },
            &VmJobOutput::Evaluated(RunOutcome {
                report: report_for(&req, 0.1),
                logs: vec![],
            })
        ));
    }
}
