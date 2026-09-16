//! Per-VM networking: a TAP on a /30, NAT through the uplink, and an
//! nftables table that lets the RLM VM reach **only** the operator's egress
//! allowlist. The sister miner guest gets no interface at all.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use proof_vm_agent::HvError;

use crate::config::{EgressAllow, HostConfig, Proto};
use crate::shell::{sh, Shell};

/// How many TAP indexes one boot will try before it gives up. The allocator
/// scans the host first, so this only covers a race with a concurrent boot
/// (two experiments starting at once can pick the same free index). It counts
/// **attempts**, so `1` means "no retry".
pub const TAP_ATTEMPTS: u32 = 16;

/// One RLM VM's network plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPlan {
    /// Host TAP device (`pfc<n>`, ≤15 chars).
    pub tap: String,
    /// Host side of the /30.
    pub host_ip: Ipv4Addr,
    /// Guest side of the /30.
    pub guest_ip: Ipv4Addr,
    /// Guest MAC (`AA:FC:...`, derived from the index).
    pub guest_mac: String,
    /// Uplink for masquerade.
    pub uplink: String,
    /// Allowlist.
    pub allow: Vec<EgressAllow>,
}

impl NetPlan {
    /// Plan for the `index`-th VM: /30 number `index` of the pool.
    #[must_use]
    pub fn for_index(cfg: &HostConfig, index: u32) -> Self {
        let base = u32::from(cfg.net_base) & !0b11;
        let net = base.wrapping_add(index.wrapping_mul(4));
        let [a, b] = [(index >> 8) & 0xff, index & 0xff];
        Self {
            tap: format!("pfc{index}"),
            host_ip: Ipv4Addr::from(net.wrapping_add(1)),
            guest_ip: Ipv4Addr::from(net.wrapping_add(2)),
            guest_mac: format!("AA:FC:00:00:{a:02X}:{b:02X}"),
            uplink: cfg.uplink.clone(),
            allow: cfg.egress_allow.clone(),
        }
    }

    /// nftables table name (one per VM so teardown is one `delete table`).
    #[must_use]
    pub fn table(&self) -> String {
        format!("proof_vm_{}", self.tap)
    }

    /// The first `pfc<n>` index at or above `from` that this host does not
    /// already have.
    ///
    /// A process-local counter is not enough to pick one. A jailed VM
    /// outlives the agent process that booted it — the jailer is handed over,
    /// not a child of the agent — so an agent restart, or a VM an earlier
    /// agent left behind (a topic VM still holding its TAP after a baseline),
    /// keeps `pfc<n>` in place while the counter starts again at zero.
    /// `ip tuntap add` on a live name is `ioctl(TUNSETIFF): Device or
    /// resource busy`, which is how a leftover topic VM took every new
    /// experiment VM off the air.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`] when `ip` fails.
    pub async fn first_free(shell: &dyn Shell, from: u32) -> Result<u32, HvError> {
        let out = sh(shell, "ip", &["-o", "link", "show"]).await?;
        let used: BTreeSet<u32> = out.stdout.lines().filter_map(tap_index).collect();
        let mut i = from;
        while used.contains(&i) {
            i = i.saturating_add(1);
        }
        Ok(i)
    }

    /// Whether this error is the host refusing a TAP name that is taken.
    ///
    /// `ip tuntap add` renders both `EBUSY` and `EEXIST` from `TUNSETIFF` as
    /// `ioctl(TUNSETIFF): Device or resource busy`, so the text is the only
    /// signal the command's exit status carries.
    #[must_use]
    pub fn name_taken(e: &HvError) -> bool {
        matches!(e, HvError::Backend(m) if m.contains("TUNSETIFF") || m.contains("resource busy"))
    }

    /// Kernel `ip=` argument giving the guest its address statically.
    #[must_use]
    pub fn boot_arg(&self) -> String {
        format!(
            "ip={}::{}:255.255.255.252::eth0:off",
            self.guest_ip, self.host_ip
        )
    }

    /// The per-VM nftables ruleset: forward from the TAP only to the
    /// allowlist (established replies back in), masquerade out the uplink,
    /// drop everything else the guest sends. Empty allowlist = no egress.
    #[must_use]
    pub fn ruleset(&self) -> String {
        use std::fmt::Write as _;
        let t = self.table();
        let tap = &self.tap;
        let mut out = format!(
            "table inet {t} {{\n  chain forward {{\n    type filter hook forward priority 0; policy accept;\n    oifname \"{tap}\" ct state established,related accept\n    oifname \"{tap}\" drop\n    iifname \"{tap}\" ct state established,related accept\n"
        );
        for a in &self.allow {
            let l4 = match (a.proto, a.port) {
                (Proto::Tcp, Some(p)) => format!(" tcp dport {p}"),
                (Proto::Udp, Some(p)) => format!(" udp dport {p}"),
                (Proto::Tcp, None) => " meta l4proto tcp".into(),
                (Proto::Udp, None) => " meta l4proto udp".into(),
                (Proto::Any, _) => String::new(),
            };
            let _ = writeln!(
                out,
                "    iifname \"{tap}\" ip daddr {}{l4} accept",
                a.cidr()
            );
        }
        let _ = write!(
            out,
            "    iifname \"{tap}\" drop\n  }}\n  chain postrouting {{\n    type nat hook postrouting priority 100; policy accept;\n    ip saddr {} oifname \"{}\" masquerade\n  }}\n}}\n",
            self.guest_ip, self.uplink
        );
        out
    }

    /// Create the TAP (owned by the jail uid so jailed Firecracker can open
    /// it), address it, enable forwarding, load the ruleset.
    ///
    /// A name this host already has is refused **by name** rather than
    /// half-built: the caller allocates a free index first
    /// ([`Self::first_free`]) and retries on this error, so a TAP that
    /// belongs to a live VM is never addressed or deleted by a boot that did
    /// not create it.
    ///
    /// Once `ip tuntap add` succeeds the interface **is** this plan's, so a
    /// later step failing (address, link, sysctl) rolls the whole plan back
    /// here. Without that, the caller's guard would release a jail for a
    /// network it was never told it owned, and the interface would outlive
    /// every record of it — consuming a name that later boots then allocate
    /// around.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`] from the first failing command; a taken name is
    /// recognisable with [`Self::name_taken`].
    pub async fn up(&self, shell: &dyn Shell, jail_uid: u32) -> Result<(), HvError> {
        let uid = jail_uid.to_string();
        let added = sh(
            shell,
            "ip",
            &[
                "tuntap", "add", "dev", &self.tap, "mode", "tap", "user", &uid,
            ],
        )
        .await;
        if let Err(e) = added {
            return Err(if Self::name_taken(&e) {
                HvError::Backend(format!(
                    "tap {} is already taken on this host ({e}); another vm holds it",
                    self.tap
                ))
            } else {
                e
            });
        }
        let rest = async {
            let cidr = format!("{}/30", self.host_ip);
            sh(shell, "ip", &["addr", "add", &cidr, "dev", &self.tap]).await?;
            sh(shell, "ip", &["link", "set", &self.tap, "up"]).await?;
            sh(shell, "sysctl", &["-q", "-w", "net.ipv4.ip_forward=1"]).await?;
            Ok(())
        }
        .await;
        if let Err(e) = rest {
            // The interface exists and nothing else knows about it yet.
            for err in self.down(shell).await {
                tracing::debug!(tap = %self.tap, "rollback of a half-built network: {err}");
            }
            return Err(e);
        }
        Ok(())
    }

    /// Load the ruleset from `ruleset_path` (written beside the jail by the
    /// caller; `nft` reads files, the [`Shell`] carries no stdin).
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`].
    pub async fn load_rules(&self, shell: &dyn Shell, ruleset_path: &str) -> Result<(), HvError> {
        sh(shell, "nft", &["-f", ruleset_path]).await?;
        Ok(())
    }

    /// Forward chains of **other** tables that drop by default (ufw's
    /// `filter FORWARD`, Docker's) **and do not accept the TAPs**, rendered
    /// `family table chain`. Every base chain on the forward hook sees the
    /// packet and one `drop` verdict wins, so the per-VM allow rules cannot
    /// rescue guest egress from such a chain: the operator has to accept the
    /// TAPs there (runbook § Egress). The fix leaves the policy at `drop` and
    /// adds an `iifname "pfc*"` accept somewhere in that table (ufw's
    /// `ufw-before-forward`, Docker's `DOCKER-USER`, iptables `-i pfc+`), so
    /// the check reads the rules: a table with such an accept is handled and
    /// not reported — the warning clears once the fix is in. Advisory only —
    /// an `nft` that cannot list or a host without those chains yields an
    /// empty list / an error the caller logs, never a failed boot.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`] when `nft -j list ruleset` fails or is not JSON.
    pub async fn foreign_forward_drops(shell: &dyn Shell) -> Result<Vec<String>, HvError> {
        let out = sh(shell, "nft", &["-j", "list", "ruleset"]).await?;
        let doc: serde_json::Value = serde_json::from_str(&out.stdout)
            .map_err(|e| HvError::Backend(format!("nft -j list ruleset: {e}")))?;
        let items = doc["nftables"].as_array().into_iter().flatten();
        let key = |v: &serde_json::Value| {
            format!(
                "{} {}",
                v["family"].as_str().unwrap_or("?"),
                v["table"].as_str().unwrap_or("?")
            )
        };
        // Tables that already accept traffic from a TAP anywhere in them.
        let handled: Vec<String> = items
            .clone()
            .filter(|item| rule_accepts_tap(&item["rule"]))
            .map(|item| key(&item["rule"]))
            .collect();
        let mut drops = Vec::new();
        for item in items {
            let chain = &item["chain"];
            let table = chain["table"].as_str().unwrap_or_default();
            if chain["hook"].as_str() != Some("forward")
                || chain["policy"].as_str() != Some("drop")
                || table.starts_with("proof_vm_")
                || handled.contains(&key(chain))
            {
                continue;
            }
            drops.push(format!(
                "{} {}",
                key(chain),
                chain["name"].as_str().unwrap_or("?")
            ));
        }
        Ok(drops)
    }

    /// Delete the table and the TAP. Errors are reported, not fatal: a
    /// half-torn network must not leave the VM record alive.
    pub async fn down(&self, shell: &dyn Shell) -> Vec<HvError> {
        let mut errs = Vec::new();
        if let Err(e) = sh(shell, "nft", &["delete", "table", "inet", &self.table()]).await {
            errs.push(e);
        }
        if let Err(e) = sh(shell, "ip", &["link", "del", &self.tap]).await {
            errs.push(e);
        }
        errs
    }
}

/// Prefix every TAP name shares (`pfc<n>`).
const TAP_PREFIX: &str = "pfc";

/// The index of a `pfc<n>` interface, from one `ip -o link show` line
/// (`3: pfc0: <BROADCAST,…> mtu …`). `None` for anything else on the host.
fn tap_index(line: &str) -> Option<u32> {
    let rest = line.split_once(": ")?.1;
    let name = rest.split([':', '@']).next()?;
    name.strip_prefix(TAP_PREFIX)?.parse().ok()
}

/// Does this `nft -j` rule accept traffic arriving on a TAP? True for an
/// `iifname` match against `pfc*` / `pfc+` / a specific `pfc<n>` (also inside
/// a set) that ends in an `accept` verdict — the shape both
/// `iptables -I … -i pfc+ -j ACCEPT` (rendered by iptables-nft as
/// `iifname "pfc*" … accept`) and a native `iifname "pfc*" accept` take.
fn rule_accepts_tap(rule: &serde_json::Value) -> bool {
    let Some(exprs) = rule["expr"].as_array() else {
        return false;
    };
    let names_tap = |right: &serde_json::Value| -> bool {
        let is_tap = |s: &str| s.starts_with(TAP_PREFIX);
        right.as_str().is_some_and(is_tap)
            || right["set"]
                .as_array()
                .is_some_and(|set| set.iter().any(|v| v.as_str().is_some_and(is_tap)))
    };
    let matches_tap = exprs.iter().any(|e| {
        let m = &e["match"];
        m["left"]["meta"]["key"].as_str() == Some("iifname")
            && matches!(m["op"].as_str(), Some("==") | None)
            && names_tap(&m["right"])
    });
    matches_tap && exprs.iter().any(|e| e.get("accept").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{FailingShell, RecordingShell};

    /// A shell that answers `ip -o link show` with a canned interface listing
    /// and fails anything else — the host state the allocator has to read.
    struct LinkListing(String);

    impl LinkListing {
        fn new(lines: &[&str]) -> Self {
            Self(lines.join("\n") + "\n")
        }
    }

    #[async_trait::async_trait]
    impl Shell for LinkListing {
        async fn run(
            &self,
            program: &str,
            args: &[String],
        ) -> Result<crate::shell::CmdOutput, HvError> {
            assert_eq!(program, "ip", "the scan must ask the host");
            assert_eq!(args, ["-o", "link", "show"]);
            Ok(crate::shell::CmdOutput {
                code: Some(0),
                stdout: self.0.clone(),
                stderr: String::new(),
            })
        }
    }

    fn cfg() -> HostConfig {
        let mut c = HostConfig::defaults();
        c.egress_allow = vec![
            EgressAllow::parse("203.0.113.10/32:443").expect("allow"),
            EgressAllow::parse("198.51.100.0/24").expect("allow"),
            EgressAllow::parse("1.1.1.1:53/udp").expect("allow"),
        ];
        c
    }

    /// The LIVE Gate 3 failure: a topic VM left over from the RLM
    /// install/baseline still holds `pfc0`, and the allocator — a counter
    /// starting again at zero after the agent restarted — named it for the
    /// next experiment VM. `ip tuntap add` answered
    /// `ioctl(TUNSETIFF): Device or resource busy`, so no miner submission
    /// ever got a `pf_` row.
    ///
    /// The allocator now asks the **host** which indexes exist, so a leftover
    /// VM is skipped instead of collided with.
    #[tokio::test]
    async fn a_leftover_topic_vm_does_not_take_the_next_index() {
        let shell = LinkListing::new(&[
            "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT",
            "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP",
            // The leftover topic VM's TAP, still up from the baseline run.
            "3: pfc0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP",
        ]);
        let next = NetPlan::first_free(&shell, 0).await.expect("scan");
        assert_eq!(next, 1, "pfc0 is taken by the leftover topic vm");
        assert_eq!(NetPlan::for_index(&cfg(), next).tap, "pfc1");

        // A host with no TAPs of ours starts at the counter's value.
        let empty = LinkListing::new(&["1: lo: <LOOPBACK,UP> mtu 65536"]);
        assert_eq!(NetPlan::first_free(&empty, 0).await.expect("scan"), 0);
        assert_eq!(NetPlan::first_free(&empty, 7).await.expect("scan"), 7);

        // A gap is used before a higher index: a torn-down VM frees its slot.
        let gap = LinkListing::new(&[
            "3: pfc0: <BROADCAST,UP> mtu 1500",
            "5: pfc2: <BROADCAST,UP> mtu 1500",
        ]);
        assert_eq!(gap_indices(&gap).await, vec![0, 2]);
        assert_eq!(NetPlan::first_free(&gap, 0).await.expect("scan"), 1);
    }

    /// The scan reads only our TAPs: an unrelated interface whose name merely
    /// starts with `pfc`-like text, or a `pfc` with a non-numeric suffix, is
    /// not an index.
    #[tokio::test]
    async fn the_scan_counts_only_pfc_indexes() {
        let shell = LinkListing::new(&[
            "3: pfc0: <BROADCAST,UP> mtu 1500",
            "4: pfc10: <BROADCAST,UP> mtu 1500",
            "5: pfconf: <BROADCAST,UP> mtu 1500",
            "6: pfcX: <BROADCAST,UP> mtu 1500",
            "7: docker0: <BROADCAST,UP> mtu 1500",
        ]);
        assert_eq!(gap_indices(&shell).await, vec![0, 10]);
        // 1..9 are free, so the next is 1 — the scan does not jump to 11.
        assert_eq!(NetPlan::first_free(&shell, 0).await.expect("scan"), 1);
    }

    /// A taken name is recognised from the kernel's own wording, which is what
    /// the boot retry keys on.
    #[test]
    fn a_taken_tap_name_is_recognised_from_the_kernels_wording() {
        let busy =
            HvError::Backend("ip exited Some(1): ioctl(TUNSETIFF): Device or resource busy".into());
        assert!(NetPlan::name_taken(&busy));
        assert!(!NetPlan::name_taken(&HvError::Backend(
            "ip exited Some(1): RTNETLINK answers: Operation not permitted".into()
        )));
        assert!(!NetPlan::name_taken(&HvError::Guest("busy".into())));
    }

    /// A step after `ip tuntap add` failing must roll the interface back.
    ///
    /// The guard's ownership flag is set from `up` returning `Ok`, so a
    /// failure after the interface exists leaves the caller believing it owns
    /// nothing — and the interface would outlive every record of it, holding
    /// a name that later boots then allocate around.
    #[tokio::test]
    async fn a_failure_after_the_tap_exists_rolls_the_interface_back() {
        for fail_on in ["ip addr add", "ip link set", "sysctl"] {
            let shell = FailingShell::failing_on(fail_on);
            let plan = NetPlan::for_index(&cfg(), 3);
            let err = plan
                .up(&shell, 65_534)
                .await
                .expect_err("injected failure after the tap exists");
            assert!(err.to_string().contains("injected failure"), "{fail_on}");
            let lines = shell.lines();
            let added = lines
                .iter()
                .position(|l| l.starts_with("ip tuntap add"))
                .expect("the tap was created");
            let after = &lines[added + 1..];
            assert!(
                after.contains(&"nft delete table inet proof_vm_pfc3".to_owned()),
                "{fail_on}: the table is rolled back: {after:?}"
            );
            assert!(
                after.contains(&"ip link del pfc3".to_owned()),
                "{fail_on}: the interface is rolled back: {after:?}"
            );
        }
        // And a failure *at* the create does not roll anything back: this
        // boot never had the interface, so there is nothing of its to remove.
        let shell = FailingShell::failing_on("ip tuntap");
        let plan = NetPlan::for_index(&cfg(), 3);
        plan.up(&shell, 65_534).await.expect_err("taken name");
        let lines = shell.lines();
        assert!(
            !lines.iter().any(|l| l.starts_with("ip link del")),
            "a name this boot never took must not be deleted: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("nft delete table")),
            "a name this boot never took must not be deleted: {lines:?}"
        );
    }

    async fn gap_indices(shell: &LinkListing) -> Vec<u32> {
        let out = shell
            .run(
                "ip",
                &["-o".to_owned(), "link".to_owned(), "show".to_owned()],
            )
            .await
            .expect("scan");
        let mut got: Vec<u32> = out.stdout.lines().filter_map(tap_index).collect();
        got.sort_unstable();
        got
    }

    #[test]
    fn plans_carve_the_pool_into_p2p_slash_30s() {
        let p0 = NetPlan::for_index(&cfg(), 0);
        assert_eq!(p0.tap, "pfc0");
        assert_eq!(p0.host_ip, Ipv4Addr::new(172, 16, 0, 1));
        assert_eq!(p0.guest_ip, Ipv4Addr::new(172, 16, 0, 2));
        assert_eq!(p0.guest_mac, "AA:FC:00:00:00:00");
        assert_eq!(
            p0.boot_arg(),
            "ip=172.16.0.2::172.16.0.1:255.255.255.252::eth0:off"
        );
        let p300 = NetPlan::for_index(&cfg(), 300);
        assert_eq!(p300.host_ip, Ipv4Addr::new(172, 16, 4, 177));
        assert_eq!(p300.guest_ip, Ipv4Addr::new(172, 16, 4, 178));
        assert_eq!(p300.guest_mac, "AA:FC:00:00:01:2C");
        assert!(p300.tap.len() <= 15);
        assert_eq!(p300.table(), "proof_vm_pfc300");
    }

    #[test]
    fn the_ruleset_allows_only_the_list_and_drops_the_rest() {
        let p = NetPlan::for_index(&cfg(), 7);
        let rules = p.ruleset();
        let want = "table inet proof_vm_pfc7 {\n  chain forward {\n    type filter hook forward priority 0; policy accept;\n    oifname \"pfc7\" ct state established,related accept\n    oifname \"pfc7\" drop\n    iifname \"pfc7\" ct state established,related accept\n    iifname \"pfc7\" ip daddr 203.0.113.10/32 tcp dport 443 accept\n    iifname \"pfc7\" ip daddr 198.51.100.0/24 accept\n    iifname \"pfc7\" ip daddr 1.1.1.1/32 udp dport 53 accept\n    iifname \"pfc7\" drop\n  }\n  chain postrouting {\n    type nat hook postrouting priority 100; policy accept;\n    ip saddr 172.16.0.30 oifname \"eth0\" masquerade\n  }\n}\n";
        assert_eq!(rules, want);
        let mut none = cfg();
        none.egress_allow.clear();
        let closed = NetPlan::for_index(&none, 0).ruleset();
        assert!(!closed.contains("daddr"), "no allow → no accept: {closed}");
        assert!(closed.contains("iifname \"pfc0\" drop"));
    }

    #[tokio::test]
    async fn up_and_down_render_the_expected_commands() {
        let shell = RecordingShell::default();
        let p = NetPlan::for_index(&cfg(), 2);
        p.up(&shell, 65534).await.expect("up");
        p.load_rules(&shell, "/srv/jailer/firecracker/x/net.nft")
            .await
            .expect("rules");
        assert!(p.down(&shell).await.is_empty());
        let calls = shell.calls();
        let flat: Vec<String> = calls.iter().map(|c| c.join(" ")).collect();
        assert!(
            flat.contains(&"ip tuntap add dev pfc2 mode tap user 65534".to_owned()),
            "{flat:?}"
        );
        assert!(
            flat.contains(&"ip addr add 172.16.0.9/30 dev pfc2".to_owned()),
            "{flat:?}"
        );
        assert!(flat.contains(&"ip link set pfc2 up".to_owned()));
        assert!(flat.contains(&"sysctl -q -w net.ipv4.ip_forward=1".to_owned()));
        assert!(flat.contains(&"nft -f /srv/jailer/firecracker/x/net.nft".to_owned()));
        assert!(flat.contains(&"nft delete table inet proof_vm_pfc2".to_owned()));
        assert!(flat.contains(&"ip link del pfc2".to_owned()));
    }

    /// Shell that answers `nft -j list ruleset` with a canned listing.
    struct NftListing(String);

    #[async_trait::async_trait]
    impl Shell for NftListing {
        async fn run(
            &self,
            program: &str,
            args: &[String],
        ) -> Result<crate::shell::CmdOutput, HvError> {
            assert_eq!(program, "nft");
            assert_eq!(args, ["-j", "list", "ruleset"]);
            Ok(crate::shell::CmdOutput {
                code: Some(0),
                stdout: self.0.clone(),
                stderr: String::new(),
            })
        }
    }

    /// A host with ufw (ip + ip6 `filter FORWARD` drop) and Docker
    /// (`DOCKER-USER`), plus one of our per-VM tables. `extra_rules` are
    /// appended as the operator's fix.
    fn ruleset(extra_rules: &str) -> String {
        format!(
            r#"{{"nftables": [
          {{"metainfo": {{"version": "1.0.9", "release_name": "Old Doc Yak #3", "json_schema_version": 1}}}},
          {{"table": {{"family": "ip", "table": "filter", "handle": 1}}}},
          {{"chain": {{"family": "ip", "table": "filter", "name": "INPUT", "handle": 1, "type": "filter", "hook": "input", "prio": 0, "policy": "drop"}}}},
          {{"chain": {{"family": "ip", "table": "filter", "name": "FORWARD", "handle": 2, "type": "filter", "hook": "forward", "prio": 0, "policy": "drop"}}}},
          {{"chain": {{"family": "ip", "table": "filter", "name": "ufw-before-forward", "handle": 9}}}},
          {{"chain": {{"family": "ip", "table": "filter", "name": "DOCKER-USER", "handle": 12}}}},
          {{"rule": {{"family": "ip", "table": "filter", "chain": "FORWARD", "handle": 13, "expr": [{{"jump": {{"target": "DOCKER-USER"}}}}]}}}},
          {{"rule": {{"family": "ip", "table": "filter", "chain": "FORWARD", "handle": 14, "expr": [{{"jump": {{"target": "ufw-before-forward"}}}}]}}}},
          {{"rule": {{"family": "ip", "table": "filter", "chain": "ufw-before-forward", "handle": 15, "expr": [{{"match": {{"op": "in", "left": {{"ct": {{"key": "state"}}}}, "right": ["related", "established"]}}}}, {{"counter": {{"packets": 0, "bytes": 0}}}}, {{"accept": null}}]}}}},
          {{"rule": {{"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 16, "expr": [{{"counter": {{"packets": 0, "bytes": 0}}}}, {{"return": null}}]}}}},
          {{"chain": {{"family": "inet", "table": "proof_vm_pfc0", "name": "forward", "handle": 30, "type": "filter", "hook": "forward", "prio": 0, "policy": "accept"}}}},
          {{"rule": {{"family": "inet", "table": "proof_vm_pfc0", "chain": "forward", "handle": 32, "expr": [{{"match": {{"op": "==", "left": {{"meta": {{"key": "iifname"}}}}, "right": "pfc0"}}}}, {{"match": {{"op": "==", "left": {{"payload": {{"protocol": "ip", "field": "daddr"}}}}, "right": {{"prefix": {{"addr": "203.0.113.10", "len": 32}}}}}}}}, {{"accept": null}}]}}}},
          {{"chain": {{"family": "inet", "table": "proof_vm_pfc0", "name": "postrouting", "handle": 31, "type": "nat", "hook": "postrouting", "prio": 100, "policy": "accept"}}}},
          {{"chain": {{"family": "ip6", "table": "filter", "name": "FORWARD", "handle": 40, "type": "filter", "hook": "forward", "prio": 0, "policy": "drop"}}}},
          {{"chain": {{"family": "inet", "table": "firewalld", "name": "filter_FORWARD", "handle": 50, "type": "filter", "hook": "forward", "prio": 10, "policy": "accept"}}}}
          {extra_rules}
        ]}}"#
        )
    }

    /// ufw's and Docker's forward chains drop by default and sit beside the
    /// per-VM tables; the preflight names exactly those, never our own
    /// tables, never chains on other hooks or with an accept policy — and
    /// stops naming a table once it accepts the TAPs (the runbook's ufw /
    /// `DOCKER-USER` fix, which leaves the policy at `drop`).
    #[tokio::test]
    async fn foreign_forward_drops_name_ufw_and_docker_until_the_taps_are_accepted() {
        let before = NetPlan::foreign_forward_drops(&NftListing(ruleset("")))
            .await
            .expect("parsed");
        assert_eq!(before, ["ip filter FORWARD", "ip6 filter FORWARD"]);

        // The ufw fix as iptables-nft renders `-A ufw-before-forward -i pfc+ -j ACCEPT`
        // (+ the return path): the ip table is handled, ip6 still is not.
        let ufw_fix = r#",
          {"rule": {"family": "ip", "table": "filter", "chain": "ufw-before-forward", "handle": 60, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "pfc*"}}, {"counter": {"packets": 0, "bytes": 0}}, {"accept": null}]}},
          {"rule": {"family": "ip", "table": "filter", "chain": "ufw-before-forward", "handle": 61, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "oifname"}}, "right": "pfc*"}}, {"match": {"op": "in", "left": {"ct": {"key": "state"}}, "right": ["related", "established"]}}, {"counter": {"packets": 0, "bytes": 0}}, {"accept": null}]}}"#;
        let after_ufw = NetPlan::foreign_forward_drops(&NftListing(ruleset(ufw_fix)))
            .await
            .expect("parsed");
        assert_eq!(after_ufw, ["ip6 filter FORWARD"], "ip filter handled");

        // The Docker fix in DOCKER-USER, spelled as a native set of TAP names.
        let docker_fix = r#",
          {"rule": {"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 70, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": {"set": ["pfc0", "pfc1"]}}}, {"accept": null}]}}"#;
        let after_docker = NetPlan::foreign_forward_drops(&NftListing(ruleset(docker_fix)))
            .await
            .expect("parsed");
        assert_eq!(after_docker, ["ip6 filter FORWARD"]);

        // Not fixes: an oifname-only rule, a TAP match that drops / returns,
        // an accept for another interface. An accept in the ip6 table fixes
        // ip6 only.
        let not_fixes = r#",
          {"rule": {"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 80, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "oifname"}}, "right": "pfc*"}}, {"accept": null}]}},
          {"rule": {"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 81, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "pfc*"}}, {"drop": null}]}},
          {"rule": {"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 82, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "pfc*"}}, {"return": null}]}},
          {"rule": {"family": "ip", "table": "filter", "chain": "DOCKER-USER", "handle": 83, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "docker0"}}, {"accept": null}]}},
          {"rule": {"family": "ip6", "table": "filter", "chain": "FORWARD", "handle": 84, "expr": [{"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "pfc*"}}, {"accept": null}]}}"#;
        let still = NetPlan::foreign_forward_drops(&NftListing(ruleset(not_fixes)))
            .await
            .expect("parsed");
        assert_eq!(
            still,
            ["ip filter FORWARD"],
            "ip6 is fixed by its own rule, ip is not"
        );

        let clean = NetPlan::foreign_forward_drops(&NftListing(
            r#"{"nftables": [{"metainfo": {}}]}"#.to_owned(),
        ))
        .await
        .expect("parsed");
        assert!(clean.is_empty());
        let err = NetPlan::foreign_forward_drops(&NftListing("not json".to_owned()))
            .await
            .expect_err("garbage");
        assert!(err.to_string().contains("nft -j list ruleset"), "{err}");
        // The recording shell (CI) answers an empty stdout: an error to log,
        // never a boot failure — `bring_up` ignores it.
        assert!(NetPlan::foreign_forward_drops(&RecordingShell::default())
            .await
            .is_err());
    }
}
