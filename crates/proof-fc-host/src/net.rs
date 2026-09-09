//! Per-VM networking: a TAP on a /30, NAT through the uplink, and an
//! nftables table that lets the RLM VM reach **only** the operator's egress
//! allowlist. The sister miner guest gets no interface at all.

use std::net::Ipv4Addr;

use proof_vm_agent::HvError;

use crate::config::{EgressAllow, HostConfig, Proto};
use crate::shell::{sh, Shell};

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
    /// # Errors
    ///
    /// [`HvError::Backend`] from the first failing command.
    pub async fn up(&self, shell: &dyn Shell, jail_uid: u32) -> Result<(), HvError> {
        let uid = jail_uid.to_string();
        sh(
            shell,
            "ip",
            &[
                "tuntap", "add", "dev", &self.tap, "mode", "tap", "user", &uid,
            ],
        )
        .await?;
        let cidr = format!("{}/30", self.host_ip);
        sh(shell, "ip", &["addr", "add", &cidr, "dev", &self.tap]).await?;
        sh(shell, "ip", &["link", "set", &self.tap, "up"]).await?;
        sh(shell, "sysctl", &["-q", "-w", "net.ipv4.ip_forward=1"]).await?;
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
    /// `filter FORWARD`, Docker's), rendered `family table chain`. Every base
    /// chain on the forward hook sees the packet and one `drop` verdict
    /// wins, so the per-VM allow rules cannot rescue guest egress from such
    /// a chain: the operator has to accept the TAPs there (runbook § Egress).
    /// Diagnostic only — an `nft` that cannot list or a host without those
    /// chains yields an empty list / an error the caller logs, never a
    /// failed boot.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`] when `nft -j list chains` fails or is not JSON.
    pub async fn foreign_forward_drops(shell: &dyn Shell) -> Result<Vec<String>, HvError> {
        let out = sh(shell, "nft", &["-j", "list", "chains"]).await?;
        let doc: serde_json::Value = serde_json::from_str(&out.stdout)
            .map_err(|e| HvError::Backend(format!("nft -j list chains: {e}")))?;
        let mut drops = Vec::new();
        for item in doc["nftables"].as_array().into_iter().flatten() {
            let chain = &item["chain"];
            let table = chain["table"].as_str().unwrap_or_default();
            if chain["hook"].as_str() != Some("forward")
                || chain["policy"].as_str() != Some("drop")
                || table.starts_with("proof_vm_")
            {
                continue;
            }
            drops.push(format!(
                "{} {table} {}",
                chain["family"].as_str().unwrap_or("?"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::RecordingShell;

    fn cfg() -> HostConfig {
        let mut c = HostConfig::defaults();
        c.egress_allow = vec![
            EgressAllow::parse("203.0.113.10/32:443").expect("allow"),
            EgressAllow::parse("198.51.100.0/24").expect("allow"),
            EgressAllow::parse("1.1.1.1:53/udp").expect("allow"),
        ];
        c
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

    /// Shell that answers `nft -j list chains` with a canned ruleset listing.
    struct NftListing(&'static str);

    #[async_trait::async_trait]
    impl Shell for NftListing {
        async fn run(
            &self,
            program: &str,
            args: &[String],
        ) -> Result<crate::shell::CmdOutput, HvError> {
            assert_eq!(program, "nft");
            assert_eq!(args, ["-j", "list", "chains"]);
            Ok(crate::shell::CmdOutput {
                code: Some(0),
                stdout: self.0.to_owned(),
                stderr: String::new(),
            })
        }
    }

    /// ufw's and Docker's forward chains drop by default and sit beside the
    /// per-VM tables; the preflight names exactly those, never our own
    /// tables, never chains on other hooks or with an accept policy.
    #[tokio::test]
    async fn foreign_forward_drops_name_ufw_and_docker_not_our_tables() {
        let listing = r#"{"nftables": [
          {"metainfo": {"version": "1.0.9", "release_name": "Old Doc Yak #3", "json_schema_version": 1}},
          {"chain": {"family": "ip", "table": "filter", "name": "INPUT", "handle": 1, "type": "filter", "hook": "input", "prio": 0, "policy": "drop"}},
          {"chain": {"family": "ip", "table": "filter", "name": "FORWARD", "handle": 2, "type": "filter", "hook": "forward", "prio": 0, "policy": "drop"}},
          {"chain": {"family": "ip", "table": "filter", "name": "ufw-before-forward", "handle": 9}},
          {"chain": {"family": "ip", "table": "filter", "name": "DOCKER-USER", "handle": 12}},
          {"chain": {"family": "inet", "table": "proof_vm_pfc0", "name": "forward", "handle": 30, "type": "filter", "hook": "forward", "prio": 0, "policy": "accept"}},
          {"chain": {"family": "inet", "table": "proof_vm_pfc0", "name": "postrouting", "handle": 31, "type": "nat", "hook": "postrouting", "prio": 100, "policy": "accept"}},
          {"chain": {"family": "ip6", "table": "filter", "name": "FORWARD", "handle": 40, "type": "filter", "hook": "forward", "prio": 0, "policy": "drop"}},
          {"chain": {"family": "inet", "table": "firewalld", "name": "filter_FORWARD", "handle": 50, "type": "filter", "hook": "forward", "prio": 10, "policy": "accept"}}
        ]}"#;
        let drops = NetPlan::foreign_forward_drops(&NftListing(listing))
            .await
            .expect("parsed");
        assert_eq!(drops, ["ip filter FORWARD", "ip6 filter FORWARD"]);
        let clean =
            NetPlan::foreign_forward_drops(&NftListing(r#"{"nftables": [{"metainfo": {}}]}"#))
                .await
                .expect("parsed");
        assert!(clean.is_empty());
        let err = NetPlan::foreign_forward_drops(&NftListing("not json"))
            .await
            .expect_err("garbage");
        assert!(err.to_string().contains("nft -j list chains"), "{err}");
        // The recording shell (CI) answers an empty stdout: an error to log,
        // never a boot failure — `bring_up` ignores it.
        assert!(NetPlan::foreign_forward_drops(&RecordingShell::default())
            .await
            .is_err());
    }
}
