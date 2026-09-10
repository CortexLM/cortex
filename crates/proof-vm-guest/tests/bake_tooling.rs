//! The operator bake tooling under `deploy/guest/` stays runnable: scripts
//! parse, the bake plans without root and refuses what it must, rootless
//! podman is pointed at paths init makes writable for the run-as user.
//! Generic guest scripts (`bake-rootfs.sh`, `init.sh`, `agent-loop.sh`) do
//! not name a harness. A versioned reference adaptor may ship under
//! `deploy/guest/runners/<id>/`. Nothing here builds an image or runs a
//! container.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proof-bake-tooling-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("dir");
    d
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn bake(args: &[&str]) -> (bool, String) {
    let out = Command::new("bash")
        .arg(repo().join("deploy/guest/bake-rootfs.sh"))
        .args(args)
        .output()
        .expect("run bake");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).expect(rel)
}

#[test]
fn guest_scripts_parse() {
    for (shell, file) in [
        ("bash", "deploy/guest/bake-rootfs.sh"),
        ("sh", "deploy/guest/init.sh"),
        ("sh", "deploy/guest/agent-loop.sh"),
        ("bash", "deploy/guest/runners/rlm_fc_in_guest_harbor/run"),
        (
            "bash",
            "deploy/guest/runners/rlm_fc_in_guest_harbor/inspect",
        ),
        ("bash", "deploy/guest/runners/rlm_fc_in_guest_harbor/lib.sh"),
        (
            "bash",
            "deploy/guest/runners/rlm_fc_in_guest_harbor/harness/run-harbor",
        ),
        (
            "bash",
            "deploy/guest/runners/rlm_fc_in_guest_harbor/tests/test_adaptor.sh",
        ),
        (
            "bash",
            "deploy/guest/runners/rlm_fc_in_guest_harbor/tests/run.sh",
        ),
    ] {
        let status = Command::new(shell)
            .arg("-n")
            .arg(repo().join(file))
            .status()
            .expect("shell");
        assert!(status.success(), "{file} does not parse under {shell} -n");
    }
    // The README skeleton is a real script: it must parse too.
    let readme = read("deploy/guest/runners/README.md");
    let skeleton = readme
        .split("```bash\n")
        .nth(1)
        .and_then(|s| s.split("```").next())
        .expect("skeleton block");
    assert!(skeleton.starts_with("#!/bin/bash"), "{skeleton}");
    let d = tmp("skeleton");
    let path = d.join("run");
    exe(&path, skeleton);
    let status = Command::new("bash").arg("-n").arg(&path).status().unwrap();
    assert!(status.success(), "README skeleton does not parse");
    assert!(
        skeleton.contains("no harness wired into this skeleton"),
        "the skeleton fails closed until the operator fills it in"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The bake plans without root and without network, carries the operator's
/// generic hooks (extra packages, overlays, chroot hooks) and refuses a
/// malformed one, a malformed runner id, and an adaptor without `run`, and
/// never prints a digest it did not compute.
#[test]
fn bake_dry_run_plans_and_refuses_what_it_must() {
    let d = tmp("dryrun");
    let agent = d.join("proof-vm-guest-agent");
    exe(&agent, "#!/bin/sh\nexit 0\n");
    let adaptor = d.join("adaptor");
    std::fs::create_dir_all(&adaptor).expect("adaptor dir");
    exe(&adaptor.join("run"), "#!/bin/sh\nexit 0\n");
    let overlay = d.join("overlay/opt/operator-tool/bin");
    std::fs::create_dir_all(&overlay).expect("overlay");
    let hook = d.join("hook.sh");
    exe(&hook, "#!/bin/sh\nexit 0\n");
    let agent_s = agent.display().to_string();
    let overlay_s = d.join("overlay").display().to_string();
    let hook_s = hook.display().to_string();
    let spec = format!("operator_runner_v0={}", adaptor.display());

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &spec,
        "--extra-pkgs",
        "python3-venv,python3-pip,git",
        "--overlay",
        &overlay_s,
        "--chroot-hook",
        &hook_s,
        "--resolver",
        "1.1.1.1",
        "--dry-run",
    ]);
    assert!(ok, "{text}");
    assert!(
        text.contains("runners          operator_runner_v0"),
        "{text}"
    );
    assert!(text.contains("python3-venv,python3-pip,git"), "{text}");
    assert!(
        text.contains(&format!("overlays         {overlay_s}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("chroot hooks     {hook_s}")),
        "{text}"
    );
    assert!(
        text.contains("rootless (crun, fuse-overlayfs, cgroupfs)"),
        "{text}"
    );
    assert!(
        text.contains(
            "runroot /run/user/1000/containers, graphroot /var/lib/proof/containers/storage"
        ),
        "{text}"
    );
    assert!(text.contains("tree budget 2560 MiB"), "{text}");
    assert!(text.contains("dry run: nothing built"), "{text}");
    assert!(
        !text.contains("digest: sha256:"),
        "a dry run computes no digest: {text}"
    );

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--overlay",
        &d.join("missing").display().to_string(),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("is not a directory"), "{text}");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--chroot-hook",
        &d.join("adaptor/run.txt").display().to_string(),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("is not an executable file"), "{text}");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--extra-pkgs",
        "python3; rm -rf /",
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("comma-separated list"), "{text}");

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &format!("Bad Id={}", adaptor.display()),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("must match"), "{text}");

    let empty = d.join("empty-adaptor");
    std::fs::create_dir_all(&empty).expect("dir");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &format!("operator_runner_v0={}", empty.display()),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("is not executable"), "{text}");

    let (ok, text) = bake(&["--dry-run"]);
    assert!(!ok);
    assert!(text.contains("--guest-agent is required"), "{text}");

    let (ok, text) = bake(&["--guest-agent", &agent_s, "--run-as-uid", "0", "--dry-run"]);
    assert!(!ok);
    assert!(text.contains("unprivileged uid"), "{text}");

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--size-mib",
        "2000",
        "--budget-mib",
        "2560",
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("must exceed"), "{text}");

    // The kernel-config check names every missing symbol.
    let cfg = d.join("kernel.config");
    std::fs::write(&cfg, "CONFIG_USER_NS=y\nCONFIG_VIRTIO_VSOCKETS=y\n").expect("cfg");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--check-kernel-config",
        &cfg.display().to_string(),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(
        text.contains("kernel config lacks") && text.contains("CONFIG_FUSE_FS"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Rootless podman's `runroot` / `graphroot` are the paths `init.sh`
/// creates and chowns to the run-as user (the `XDG_RUNTIME_DIR` tmpfs and
/// the scratch drive) — never the root-owned `/run/containers/storage` or
/// `/var/lib/containers/storage` on the read-only rootfs, which the
/// unprivileged runner cannot initialise. Checked on the script text the
/// bake writes and the init that prepares the guest, for the default uid
/// and for another one.
#[test]
fn podman_storage_points_at_paths_init_makes_writable_for_the_runner() {
    let bake_sh = read("deploy/guest/bake-rootfs.sh");
    let init_sh = read("deploy/guest/init.sh");
    let conf_line = |key: &str| -> String {
        bake_sh
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{key} = ")))
            .unwrap_or_else(|| panic!("{key} in storage.conf"))
            .trim()
            .to_owned()
    };
    assert_eq!(conf_line("runroot"), "runroot = \"$PODMAN_RUNROOT\"");
    assert_eq!(conf_line("graphroot"), "graphroot = \"$PODMAN_GRAPHROOT\"");
    assert_eq!(
        conf_line("rootless_storage_path"),
        "rootless_storage_path = \"$PODMAN_GRAPHROOT\""
    );
    assert!(
        bake_sh.contains("PODMAN_RUNROOT=\"/run/user/$RUN_AS_UID/containers\""),
        "runroot under the run-as user's XDG_RUNTIME_DIR"
    );
    assert!(
        bake_sh.contains("PODMAN_GRAPHROOT=\"/var/lib/proof/containers/storage\""),
        "graphroot on the scratch drive"
    );
    for forbidden in [
        "runroot = \"/run/containers/storage\"",
        "graphroot = \"/var/lib/containers/storage\"",
        "/run/containers/storage",
        "/var/lib/containers/storage",
    ] {
        assert!(
            !bake_sh.contains(forbidden),
            "{forbidden:?} is a root-owned / read-only path"
        );
    }
    // init.sh creates both, owned by the run-as user, before the agent.
    assert!(
        init_sh.contains("mkdir -p \"/run/user/$PROOF_GUEST_RUN_AS_UID/containers\""),
        "init creates the runroot"
    );
    assert!(
        init_sh.contains(
            "chown -R \"$PROOF_GUEST_RUN_AS_UID:$PROOF_GUEST_RUN_AS_GID\" \"/run/user/$PROOF_GUEST_RUN_AS_UID\""
        ),
        "init chowns the runtime dir tree"
    );
    assert!(
        init_sh.contains("\"$SCRATCH/containers/storage\""),
        "init creates the graphroot on scratch"
    );
    let scratch_chown = init_sh
        .lines()
        .find(|l| l.starts_with("chown ") && l.contains("\"$SCRATCH/containers/storage\""))
        .expect("init chowns the graphroot");
    assert!(scratch_chown.contains("$PROOF_GUEST_RUN_AS_UID:$PROOF_GUEST_RUN_AS_GID"));
    assert!(
        init_sh.contains("SCRATCH=/var/lib/proof"),
        "the scratch mount is what the graphroot lives under"
    );
    let agent_before = init_sh.find("starting proof-vm-guest-agent").unwrap();
    let runroot_at = init_sh
        .find("/run/user/$PROOF_GUEST_RUN_AS_UID/containers")
        .unwrap();
    let graphroot_at = init_sh.find("$SCRATCH/containers/storage").unwrap();
    assert!(runroot_at < agent_before && graphroot_at < agent_before);

    // The plan shows the paths for whatever uid the operator picks.
    let d = tmp("uid");
    let agent = d.join("proof-vm-guest-agent");
    exe(&agent, "#!/bin/sh\nexit 0\n");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent.display().to_string(),
        "--run-as-uid",
        "4242",
        "--dry-run",
    ]);
    assert!(ok, "{text}");
    assert!(
        text.contains("run-as uid 4242, runroot /run/user/4242/containers, graphroot /var/lib/proof/containers/storage"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Generic bake/init/agent-loop scripts stay generalist: they do not name a
/// harness. A versioned **reference adaptor** may ship under
/// `deploy/guest/runners/<id>/` so operators can bake it; it is not compiled
/// into any Proof binary. Challenge task lists still do not belong in git.
#[test]
fn deploy_guest_names_no_harness_or_benchmark() {
    let generic = [
        "deploy/guest/bake-rootfs.sh",
        "deploy/guest/init.sh",
        "deploy/guest/agent-loop.sh",
    ];
    for rel in generic {
        let lower = read(rel).to_ascii_lowercase();
        for forbidden in [
            "harbor",
            "tb4",
            "terminal-bench",
            "terminal bench",
            "tbench",
            "swe-bench",
            "verifier_result",
            "rewards.reward",
        ] {
            assert!(
                !lower.contains(forbidden),
                "{rel} names {forbidden:?}; generic guest scripts stay harness-agnostic"
            );
        }
    }

    let adaptor = repo().join("deploy/guest/runners/rlm_fc_in_guest_harbor");
    for required in [
        "run",
        "inspect",
        "README.md",
        "harness/run-harbor",
        "harness/summarize.py",
        "harness/filter_tasks.py",
        "harness/duration_hints.json",
        "harness/pack_filter.example.json",
        "harness/ensure_verifier.py",
        "harness/rewrite_network.py",
        "harness/proof_python_agent.py",
        "resolve_harness.py",
    ] {
        let p = adaptor.join(required);
        assert!(p.is_file(), "reference adaptor missing {}", p.display());
    }
    assert!(
        adaptor.join("run").metadata().unwrap().permissions().mode() & 0o111 != 0,
        "reference adaptor run must be executable"
    );
}

/// Rootful overlay path: init starts a container engine when one is present;
/// bake does not alias `docker-compose` to `podman-compose` (that alias
/// breaks Compose v2 / Harbor docker env).
#[test]
fn guest_prefers_real_docker_and_does_not_alias_compose() {
    let bake_sh = read("deploy/guest/bake-rootfs.sh");
    let init_sh = read("deploy/guest/init.sh");
    assert!(
        !bake_sh.contains("ln -s podman-compose"),
        "docker-compose must not be aliased to podman-compose"
    );
    assert!(
        bake_sh.contains("Do not alias docker-compose to podman-compose"),
        "bake must document why the compose alias is gone"
    );
    assert!(
        init_sh.contains("command -v dockerd"),
        "init starts a rootful engine when dockerd is on PATH"
    );
    assert!(
        init_sh.contains("$SCRATCH/docker"),
        "engine store must live on the scratch drive, not the read-only rootfs"
    );
}

/// The Harbor reference adaptor's unit tests (no Harbor CLI, no podman).
#[test]
fn rlm_fc_in_guest_harbor_adaptor_tests() {
    let status = Command::new("bash")
        .arg(repo().join("deploy/guest/runners/rlm_fc_in_guest_harbor/tests/run.sh"))
        .status()
        .expect("run adaptor tests");
    assert!(status.success(), "rlm_fc_in_guest_harbor tests failed");
}
