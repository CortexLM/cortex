//! Host memory the agent admits VM boots against.
//!
//! A **count** cap is not a capacity cap. Gate 4 started two 8192 MiB
//! experiment VMs beside the topic's 8192 MiB RLM VM on a 16 GiB host: the
//! kernel OOM-killed the guests and **both** submissions answered 503 with no
//! row. Refusing one request with `503 capacity` is strictly better than
//! losing both, so a boot that would not fit is refused before any jail
//! exists — the same fail-closed shape as every other admission here.
//!
//! Nothing here sizes a VM. The numbers are the operator's (`mem_mib` per
//! spec, `PROOF_VM_AGENT_MEMORY_RESERVE_MIB`); this only decides whether the
//! host can carry what was asked for.

use proof_vm_proto::VmRecord;

/// Where the host's RAM is read from.
pub const MEMINFO: &str = "/proc/meminfo";

/// Host RAM and the headroom the operator keeps out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Host RAM (`MemTotal`), MiB.
    pub total_mib: u64,
    /// Headroom kept for the OS, the agent, and per-VM process overhead, MiB.
    ///
    /// `0` is the default because a host that has been running a topic VM
    /// beside one experiment VM is already at `total` and must keep working:
    /// a non-zero default would refuse a shape that demonstrably fits.
    pub reserve_mib: u64,
}

impl MemoryBudget {
    /// A budget that admits everything — the value a state built without an
    /// operator budget carries, so unit tests are not tied to the box they
    /// run on. The `proof-vm-orchestrator` binary always sets a real one.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            total_mib: u64::MAX,
            reserve_mib: 0,
        }
    }

    /// Whether this budget admits everything (see [`Self::unlimited`]).
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.total_mib == u64::MAX && self.reserve_mib == 0
    }

    /// Read the host's RAM from [`MEMINFO`].
    ///
    /// # Errors
    ///
    /// When the file cannot be read or carries no `MemTotal`: a host that
    /// cannot prove it has room does not get to boot VMs on a guess.
    pub fn read(reserve_mib: u64) -> Result<Self, String> {
        let body = std::fs::read_to_string(MEMINFO).map_err(|e| format!("read {MEMINFO}: {e}"))?;
        Self::from_meminfo(&body, reserve_mib)
    }

    /// [`Self::read`] over a `/proc/meminfo` body.
    ///
    /// Split out so a test can drive the real parse — including the rounding
    /// to the host's purchased size — without a host to read from.
    ///
    /// # Errors
    ///
    /// When the body carries no `MemTotal`.
    pub fn from_meminfo(body: &str, reserve_mib: u64) -> Result<Self, String> {
        let total_mib =
            parse_mem_total_mib(body).ok_or_else(|| format!("{MEMINFO} carries no MemTotal"))?;
        Ok(Self {
            total_mib: nominal_total_mib(total_mib),
            reserve_mib,
        })
    }

    /// Most VM memory this host admits at once.
    #[must_use]
    pub fn ceiling_mib(&self) -> u64 {
        self.total_mib.saturating_sub(self.reserve_mib)
    }

    /// Whether a VM of `want_mib` fits beside the live ones.
    ///
    /// **Every** live VM counts, topic VMs included: the RLM VM is resident
    /// for the topic's whole life and is not free capacity. An experiment
    /// budget that ignores it is how Gate 4 oversubscribed.
    ///
    /// # What this is, and what it is not
    ///
    /// This compares **configured** guest memory against the host's. It is a
    /// guard against the oversubscription that actually happened — three
    /// 8 GiB guests on a 16 GiB host — and it is deliberately **not** a
    /// residency model:
    ///
    /// - A guest's RAM is lazily populated. Firecracker maps the region; the
    ///   guest touches pages as it works. Gate 3 proves it: a paid run on a
    ///   topic VM beside one experiment VM is 8 GiB + 8 GiB + the 4 GiB
    ///   **sister** guest = 20 GiB of configured memory on a 16 GiB host, and
    ///   it passes.
    /// - The **sister** guest is booted by the hypervisor inside a job and
    ///   never enters this count, for that reason: counting it would refuse
    ///   the proven Gate 3 shape.
    ///
    /// So do not "fix" this by summing every guest the host could ever boot:
    /// that is the change that would take the working shape offline, which is
    /// exactly the regression Greptile caught in `f0800353`. The guard is
    /// sized to refuse the observed failure with margin, not to model the
    /// kernel.
    ///
    /// # Errors
    ///
    /// The refusal, naming what holds the memory, what was asked for, and the
    /// ceiling — so an operator reads why without reaching for `free`.
    pub fn fits(&self, live: &[VmRecord], want_mib: u32) -> Result<(), String> {
        if self.is_unlimited() {
            return Ok(());
        }
        let used: u64 = live.iter().map(|r| u64::from(r.mem_mib)).sum();
        let want = u64::from(want_mib);
        let ceiling = self.ceiling_mib();
        if used.saturating_add(want) > ceiling {
            let holders: Vec<String> = live
                .iter()
                .map(|r| {
                    format!(
                        "{} {} ({} MiB)",
                        if r.experiment.is_some() {
                            "experiment"
                        } else {
                            "topic"
                        },
                        r.handle.vm_id,
                        r.mem_mib
                    )
                })
                .collect();
            let free = ceiling.saturating_sub(used);
            return Err(format!(
                "host memory: {want} MiB requested, {free} MiB free of the {ceiling} MiB VM \
                 ceiling ({total} MiB total − {reserve} MiB reserve); {used} MiB is held by {} \
                 live vm(s) [{}]. The vm was not booted — refusing here keeps the running vms \
                 alive instead of letting the host OOM-kill them. Free a vm, retry when one \
                 finishes, or lower the ask (the signed mem_mib / \
                 PROOF_VM_AGENT_EXPERIMENT_MAX_MEM_MIB)",
                live.len(),
                holders.join(", "),
                total = self.total_mib,
                reserve = self.reserve_mib,
            ));
        }
        Ok(())
    }
}

/// `MemTotal` from a `/proc/meminfo` body, in MiB.
///
/// `MemTotal:       16326344 kB` — the kernel reports KiB, so this floors to
/// whole MiB. A missing or unparseable line is `None`, never a guess.
#[must_use]
pub fn parse_mem_total_mib(body: &str) -> Option<u64> {
    let line = body.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

/// `MemTotal` rounded up to the whole GiB the host was sold as.
///
/// `MemTotal` is the RAM the kernel can hand out, **not** the RAM the host
/// has: the kernel keeps a slice for itself, so a 16 GiB droplet reports
/// `16326344 kB` = `15_943` MiB, ~441 MiB short. Sizing VMs against the raw
/// figure refuses shapes the operator sized for and that demonstrably run —
/// the proven Gate 3 pair (an 8192 MiB topic VM beside an 8192 MiB experiment
/// VM) is `16_384` MiB of guest memory on exactly that host, and a raw
/// comparison took it offline.
///
/// Guest RAM is also lazily populated: Firecracker maps the region but the
/// guest touches pages as it works, so a nominal 16 GiB host carries two
/// 8 GiB guests (Gate 3 passes) and does **not** carry three (Gate 4: the
/// kernel OOM-killed every guest at ~154% of `MemTotal`). Rounding to the
/// purchased size keeps the first and still refuses the second, without an
/// invented tolerance: RAM ships in whole GiB, so rounding restores the
/// operator's number and nothing more.
///
/// A host whose `MemTotal` is already a whole GiB is unchanged.
#[must_use]
pub fn nominal_total_mib(mem_total_mib: u64) -> u64 {
    const GIB: u64 = 1024;
    if mem_total_mib == 0 {
        return 0;
    }
    mem_total_mib.div_ceil(GIB) * GIB
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_experiment::{ExperimentSpec, PackRef};
    use proof_rlm::{RetainPolicy, SandboxPolicy, VmHandle};
    use proof_vm_proto::{VmRecord, VmState};

    fn vm(id: &str, mem_mib: u32, experiment: bool) -> VmRecord {
        VmRecord {
            handle: VmHandle {
                topic_id: "tb4".into(),
                vm_id: id.into(),
            },
            image_digest: format!("sha256:{}", "ab".repeat(32)),
            vcpus: 4,
            mem_mib,
            sandbox: SandboxPolicy {
                firecracker_required: true,
                deadline_s: 60,
            },
            retain: RetainPolicy::Destroy,
            state: VmState::Running,
            experiment: experiment.then(|| ExperimentSpec {
                runner: "placeholder_runner".into(),
                pack: PackRef {
                    path: None,
                    digest: format!("sha256:{}", "cd".repeat(32)),
                },
                disk_mib: 16_384,
            }),
        }
    }

    /// Gate 3's shape must keep working: a topic VM beside one experiment VM
    /// fills a 16 GiB host exactly, and that shape demonstrably passes.
    #[test]
    fn the_gate3_shape_still_fits() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        let topic = vm("tb4-0007", 8_192, false);
        assert!(budget.fits(std::slice::from_ref(&topic), 8_192).is_ok());
        assert!(
            budget
                .fits(&[topic, vm("tb4-x0008", 8_192, true)], 8_192)
                .is_err(),
            "a third 8 GiB vm cannot fit a 16 GiB host"
        );
    }

    /// Gate 4's shape is refused **before** a jail exists, with the numbers.
    #[test]
    fn the_gate4_oversubscription_is_refused_by_name() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        let live = [vm("tb4-0007", 8_192, false), vm("tb4-x0008", 8_192, true)];
        let err = budget
            .fits(&live, 8_192)
            .expect_err("two experiments beside a topic do not fit");
        for want in [
            "16384 MiB is held",
            "tb4-0007",
            "tb4-x0008",
            "topic tb4-0007 (8192 MiB)",
            "experiment tb4-x0008 (8192 MiB)",
            "8192 MiB requested",
            "0 MiB free",
            "16384 MiB VM ceiling",
        ] {
            assert!(err.contains(want), "the refusal names {want:?}: {err}");
        }
    }

    /// The topic VM is what makes Gate 4 not fit: the same two experiments on
    /// a host with no RLM VM up would fit, which is exactly the mistake a
    /// count-only cap makes.
    #[test]
    fn the_guard_counts_the_topic_vm_not_just_experiments() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        let experiments_only = [vm("tb4-x0008", 8_192, true)];
        assert!(
            budget.fits(&experiments_only, 8_192).is_ok(),
            "two experiments alone fit"
        );
        let with_topic = [vm("tb4-0007", 8_192, false), vm("tb4-x0008", 8_192, true)];
        assert!(
            budget.fits(&with_topic, 8_192).is_err(),
            "the resident RLM VM is not free capacity"
        );
    }

    /// The reserve is the operator's headroom, and it lowers the ceiling.
    #[test]
    fn the_reserve_lowers_the_ceiling() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 1_024,
        };
        assert_eq!(budget.ceiling_mib(), 15_360);
        let topic = vm("tb4-0007", 8_192, false);
        assert!(
            budget.fits(&[topic], 8_192).is_err(),
            "a 1 GiB reserve refuses the exact-fit shape, as the operator asked"
        );
    }

    /// `unlimited` admits everything, so a state built without an operator
    /// budget behaves exactly as it did before this guard existed.
    #[test]
    fn an_unset_budget_admits_everything() {
        let budget = MemoryBudget::unlimited();
        assert!(budget.is_unlimited());
        let live: Vec<VmRecord> = (0..8)
            .map(|i| vm(&format!("vm-{i}"), 32_768, true))
            .collect();
        assert!(budget.fits(&live, 32_768).is_ok());
    }

    #[test]
    fn memtotal_is_read_in_mib_and_never_guessed() {
        let body = "MemTotal:       16326344 kB\nMemFree:         1234567 kB\n";
        assert_eq!(parse_mem_total_mib(body), Some(15_943));
        assert_eq!(parse_mem_total_mib("MemFree: 1 kB\n"), None);
        assert_eq!(parse_mem_total_mib("MemTotal: not-a-number kB\n"), None);
        assert_eq!(parse_mem_total_mib(""), None);
    }

    /// The kernel does not hand out the RAM the host was sold: a 16 GiB
    /// droplet reports `MemTotal: 16326344 kB` = `15_943` MiB, ~441 MiB short.
    ///
    /// Sizing against the raw figure refuses the **proven Gate 3 pair** — an
    /// 8192 MiB topic VM beside an 8192 MiB experiment VM, exactly the
    /// workload that passes on this host — so the budget rounds to the
    /// purchased whole GiB. This is the regression Greptile caught: the
    /// guard must not take a working shape offline.
    ///
    /// This drives [`MemoryBudget::from_meminfo`], the real read path, so it
    /// fails if the rounding is dropped — a test that built the budget by
    /// hand would pass with the bug still in.
    #[test]
    fn a_nominal_host_keeps_the_proven_gate3_pair_admitted() {
        let body = "MemTotal:       16326344 kB\n";
        assert_eq!(
            parse_mem_total_mib(body),
            Some(15_943),
            "the raw figure is short of 16 GiB"
        );

        let budget = MemoryBudget::from_meminfo(body, 0).expect("parsed");
        assert_eq!(
            budget.total_mib, 16_384,
            "the budget must round to the sold size, not the raw figure"
        );

        let topic = vm("tb4-0007", 8_192, false);
        assert!(
            budget.fits(std::slice::from_ref(&topic), 8_192).is_ok(),
            "the proven Gate 3 pair must stay admitted on a nominal 16 GiB host"
        );
        assert!(
            budget
                .fits(&[topic, vm("tb4-x0008", 8_192, true)], 8_192)
                .is_err(),
            "and Gate 4's third guest is still refused: rounding restores the \
             operator's number, it does not invent headroom"
        );
    }

    /// The shape the Owner is tipping staging to for the Gate 4 retry:
    /// `experiment_mem_mib: 4096`, so a 16 GiB host runs the resident 8 GiB
    /// topic VM **beside two 4 GiB experiment VMs** — 16,384 MiB, exactly the
    /// ceiling.
    ///
    /// This is the retry Gate 4 depends on, so it is pinned here: an
    /// off-by-one or a reserve default would refuse it and Gate 4 would stall
    /// on the admission guard instead of the OOM.
    #[test]
    fn the_tipped_gate4_retry_shape_is_admitted() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        let topic = vm("tb4-0007", 8_192, false);
        let first = vm("tb4-x0008", 4_096, true);
        // Topic + one experiment.
        assert!(
            budget.fits(&[topic.clone(), first.clone()], 4_096).is_ok(),
            "the first 4 GiB experiment fits beside the topic vm"
        );
        // Topic + two experiments = exactly the ceiling: still admitted.
        assert!(
            budget.fits(&[topic.clone(), first.clone()], 4_096).is_ok(),
            "the second 4 GiB experiment fills the host exactly and must be admitted"
        );
        let live = [topic.clone(), first.clone(), vm("tb4-x0009", 4_096, true)];
        let used: u64 = live.iter().map(|r| u64::from(r.mem_mib)).sum();
        assert_eq!(used, 16_384, "the tipped shape is an exact fit");
        // A third experiment has nothing left.
        assert!(
            budget.fits(&live, 4_096).is_err(),
            "a third experiment has no memory left"
        );
    }

    /// The refusal names the **free** memory and what holds it, so an operator
    /// reading a 503 knows what to free without reaching for `free` — and it
    /// says the refusal is what keeps the running VMs alive.
    #[test]
    fn the_refusal_names_free_memory_and_its_holders() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        let live = [vm("tb4-0007", 8_192, false), vm("tb4-x0008", 4_096, true)];
        let err = budget.fits(&live, 8_192).expect_err("no room");
        for want in [
            "8192 MiB requested",
            "4096 MiB free",
            "16384 MiB VM ceiling",
            "12288 MiB is held",
            "topic tb4-0007 (8192 MiB)",
            "experiment tb4-x0008 (4096 MiB)",
            "OOM-kill",
        ] {
            assert!(err.contains(want), "the refusal names {want:?}: {err}");
        }
    }

    /// The two capacity refusals are distinguishable: this one is about host
    /// RAM, and the count cap (`max_experiment_vms`) has its own wording. An
    /// operator must not read "retry when one finishes" for a host that is
    /// simply too small for the shape.
    #[test]
    fn the_memory_refusal_does_not_read_as_the_count_cap() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        // A shape that genuinely does not fit: the topic VM plus a 32 GiB ask
        // on a 16 GiB host. (A lone 8 GiB topic VM has room for an 8 GiB
        // experiment, so that shape is admitted and cannot be the probe.)
        let live = [vm("tb4-0007", 8_192, false)];
        let err = budget.fits(&live, 32_768).expect_err("no room");
        assert!(
            !err.contains("PROOF_VM_AGENT_MAX_EXPERIMENT_VMS"),
            "the memory refusal must not be mistaken for the count cap: {err}"
        );
        assert!(
            err.contains("host memory"),
            "it names the constraint it is about: {err}"
        );
    }

    /// The raw figure really is what refuses the working shape — the reason
    /// the rounding exists, pinned so it cannot be dropped as "just noise".
    #[test]
    fn the_raw_figure_would_refuse_the_working_shape() {
        let raw_budget = MemoryBudget {
            total_mib: 15_943,
            reserve_mib: 0,
        };
        let topic = vm("tb4-0007", 8_192, false);
        assert!(
            raw_budget
                .fits(std::slice::from_ref(&topic), 8_192)
                .is_err(),
            "the unrounded figure refuses the shape that demonstrably runs"
        );
    }

    /// The guard counts **configured** guest memory, and the sister guest is
    /// deliberately not in it — the boundary, pinned so it is not "fixed"
    /// into re-breaking Gate 3.
    ///
    /// A paid run on the proven Gate 3 shape is 8 GiB topic + 8 GiB
    /// experiment + the 4 GiB sister = 20 GiB of configured memory on a
    /// 16 GiB host, and it **passes**: guest RAM is lazily populated, so the
    /// configured sum is not the resident set. Counting the sister here would
    /// refuse that shape.
    #[test]
    fn the_guard_is_not_a_residency_model() {
        let budget = MemoryBudget {
            total_mib: 16_384,
            reserve_mib: 0,
        };
        // The pair the host really runs — sister excluded, as it is in the
        // agent (the hypervisor boots it inside a job, never in `state.vms`).
        let live = [vm("tb4-0007", 8_192, false), vm("tb4-x0008", 8_192, true)];
        assert!(
            budget.fits(&live, 8_192).is_err(),
            "the third configured guest is refused"
        );
        // With the sister's 4 GiB added, the same live set already exceeds a
        // 16 GiB host *before* the third guest — which is why the sister is
        // not counted. If a change makes this fail, it has turned the guard
        // into a residency model and taken Gate 3 with it.
        let with_sister: u64 = live.iter().map(|r| u64::from(r.mem_mib)).sum::<u64>() + 4_096;
        assert!(
            with_sister > budget.ceiling_mib(),
            "configured memory with the sister exceeds the host, yet the shape runs"
        );
    }

    /// Rounding is a no-op on a host that already reports a whole GiB, and
    /// zero stays zero (no host size is invented).
    #[test]
    fn rounding_restores_the_sold_size_and_nothing_more() {
        assert_eq!(nominal_total_mib(16_384), 16_384);
        assert_eq!(nominal_total_mib(32_768), 32_768);
        assert_eq!(nominal_total_mib(0), 0);
        // 8 GiB sold, ~7.8 GiB visible.
        assert_eq!(nominal_total_mib(7_975), 8_192);
        // A smaller host is not rounded *up* into one that fits more than it has.
        assert_eq!(nominal_total_mib(1_024), 1_024);
    }
}
