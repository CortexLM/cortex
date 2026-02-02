//! Danger detection logic for the policy engine.

use crate::command::ParsedCommand;
use crate::config::PolicyConfig;
use crate::danger::{DangerCategory, DangerDetection};

/// Helper functions for danger detection.
pub(crate) struct DetectionHelper<'a> {
    pub config: &'a PolicyConfig,
}

impl<'a> DetectionHelper<'a> {
    /// Create a new detection helper.
    pub fn new(config: &'a PolicyConfig) -> Self {
        Self { config }
    }

    /// Normalize a path for comparison.
    pub fn normalize_path(path: &str) -> String {
        let mut normalized = path.replace("//", "/");

        // Handle trailing slashes
        while normalized.len() > 1 && normalized.ends_with('/') {
            normalized.pop();
        }

        // Expand ~ to /home or user directory indicator
        if normalized.starts_with('~') {
            normalized = normalized.replacen('~', "/home", 1);
        }

        // Handle Windows paths
        normalized = normalized.replace('\\', "/");

        normalized
    }

    /// Check if a path is considered sensitive.
    pub fn is_sensitive_path(&self, path: &str) -> bool {
        let normalized = Self::normalize_path(path);

        // Check exact matches and prefixes
        for sensitive in &self.config.sensitive_paths {
            let sens_normalized = Self::normalize_path(sensitive);

            // Exact match
            if normalized == sens_normalized {
                return true;
            }

            // Path is inside sensitive directory
            if normalized.starts_with(&format!("{sens_normalized}/")) {
                return true;
            }

            // Sensitive path is root-level
            if sens_normalized == "/" && (normalized == "/" || normalized.is_empty()) {
                return true;
            }
        }

        false
    }

    /// Check if a path refers to a block device.
    pub fn is_block_device(&self, path: &str) -> bool {
        for pattern in &self.config.block_device_patterns {
            if path.starts_with(pattern) || path.contains(pattern) {
                return true;
            }
        }
        false
    }

    /// Check for destructive file operations.
    pub fn check_destructive_file_ops(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();

        match prog {
            "rm" => {
                // Check for recursive flag
                let has_recursive = parsed.has_flag(Some('r'), Some("recursive"))
                    || parsed.has_flag(Some('R'), None);

                // Check for force flag
                let has_force = parsed.has_flag(Some('f'), Some("force"));

                if has_recursive {
                    // Check if targeting sensitive paths
                    for arg in &parsed.args {
                        let normalized = Self::normalize_path(arg);
                        if self.is_sensitive_path(&normalized) || normalized == "/" {
                            *detection = DangerDetection::dangerous(
                                DangerCategory::DestructiveFileOp,
                                format!("rm with recursive flag targeting sensitive path: {arg}"),
                                10,
                                false,
                            );
                            return;
                        }
                    }

                    // Recursive deletion is always at least a warning
                    if has_force {
                        detection.add_category(
                            DangerCategory::DestructiveFileOp,
                            "rm -rf detected (recursive force delete)",
                        );
                        detection.is_dangerous = true;
                        detection.severity = 8;
                        detection.context_mitigatable = true;
                    }
                }
            }

            "shred" | "wipe" | "srm" => {
                // Secure delete commands
                for arg in &parsed.args {
                    let normalized = Self::normalize_path(arg);
                    if self.is_sensitive_path(&normalized) {
                        *detection = DangerDetection::dangerous(
                            DangerCategory::DestructiveFileOp,
                            format!("secure delete on sensitive path: {arg}"),
                            10,
                            false,
                        );
                        return;
                    }
                }
            }

            "mv" | "cp" => {
                // Check if overwriting sensitive paths
                let positional = parsed.positional_args();
                if let Some(dest) = positional.last() {
                    let normalized = Self::normalize_path(dest);
                    if self.is_sensitive_path(&normalized) && !normalized.starts_with("/tmp") {
                        *detection = DangerDetection::dangerous(
                            DangerCategory::DestructiveFileOp,
                            format!("{prog} to sensitive destination: {dest}"),
                            7,
                            true,
                        );
                    }
                }
            }

            "truncate" => {
                // Truncating files can be destructive
                if let Some(file) = parsed.get_flag_value(Some('s'), Some("size"))
                    && file == "0"
                {
                    detection.add_category(
                        DangerCategory::DestructiveFileOp,
                        "truncating file to zero size",
                    );
                    detection.is_dangerous = true;
                    detection.severity = 5;
                    detection.context_mitigatable = true;
                }
            }

            _ => {}
        }
    }

    /// Check for disk/device operations.
    pub fn check_disk_operations(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let prog = parsed.program_basename.as_str();

        // Direct disk manipulation commands
        let disk_commands = [
            "dd",
            "fdisk",
            "gdisk",
            "parted",
            "mkfs",
            "mkfs.ext4",
            "mkfs.ext3",
            "mkfs.xfs",
            "mkfs.btrfs",
            "mkfs.ntfs",
            "mkfs.vfat",
            "mkswap",
            "swapon",
            "swapoff",
            "hdparm",
            "sdparm",
            "blkdiscard",
            "wipefs",
            "badblocks",
            "e2fsck",
            "fsck",
            "tune2fs",
            "resize2fs",
            "lvremove",
            "vgremove",
            "pvremove",
            "lvcreate",
            "vgcreate",
            "pvcreate",
            "cryptsetup",
            "losetup",
            "mdadm",
            "dmsetup",
        ];

        if disk_commands.contains(&prog) || prog.starts_with("mkfs.") {
            // Check if operating on actual block devices
            for arg in &parsed.args {
                if self.is_block_device(arg) {
                    *detection = DangerDetection::dangerous(
                        DangerCategory::DiskOperation,
                        format!("{prog} operating on block device: {arg}"),
                        10,
                        false,
                    );
                    return;
                }
            }

            // Even without block device, these commands are risky
            detection.add_category(
                DangerCategory::DiskOperation,
                &format!("disk manipulation command: {prog}"),
            );
            detection.is_dangerous = true;
            detection.severity = 8;
            detection.context_mitigatable = true;
        }

        // Special handling for dd
        if prog == "dd" {
            let has_dangerous_target = parsed.args.iter().any(|arg| {
                if let Some(of_value) = arg.strip_prefix("of=") {
                    return self.is_block_device(of_value) || self.is_sensitive_path(of_value);
                }
                false
            });

            if has_dangerous_target {
                *detection = DangerDetection::dangerous(
                    DangerCategory::DiskOperation,
                    "dd writing to block device or sensitive path",
                    10,
                    false,
                );
            }
        }
    }

    /// Check for privilege escalation commands.
    pub fn check_privilege_escalation(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();

        let priv_commands = [
            "sudo", "doas", "su", "pkexec", "runuser", "setpriv", "runas",
        ];

        if priv_commands.contains(&prog) && !self.config.allow_privilege_escalation {
            *detection = DangerDetection::dangerous(
                DangerCategory::PrivilegeEscalation,
                format!("privilege escalation via {prog}"),
                9,
                false,
            );
        }

        // Also check for setuid/setgid manipulation
        if prog == "chmod"
            && (parsed.has_arg("+s") || parsed.has_arg("u+s") || parsed.has_arg("g+s"))
        {
            *detection = DangerDetection::dangerous(
                DangerCategory::PrivilegeEscalation,
                "setting setuid/setgid bit",
                9,
                false,
            );
        }

        // Check for capability manipulation
        if prog == "setcap" || prog == "getcap" {
            detection.add_category(
                DangerCategory::PrivilegeEscalation,
                &format!("capability manipulation via {prog}"),
            );
            detection.is_dangerous = true;
            detection.severity = 8;
            detection.context_mitigatable = true;
        }
    }

    /// Check for fork bomb patterns.
    pub fn check_fork_bomb(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let raw = &parsed.raw;

        // Classic fork bomb patterns
        let fork_bomb_patterns = [
            ":(){ :|:& };:",
            ":(){ :|:& };",
            ":(){:|:&};:",
            ".LiSt(){.LiSt|.LiSt&};.LiSt",
            "bomb() { bomb | bomb & }; bomb",
            "fork() { fork | fork & }; fork",
            "forkbomb(){ forkbomb|forkbomb & };forkbomb",
            "%0|%0", // Windows fork bomb
        ];

        for pattern in fork_bomb_patterns {
            if raw.contains(pattern) {
                *detection = DangerDetection::dangerous(
                    DangerCategory::ForkBomb,
                    "fork bomb pattern detected",
                    10,
                    false,
                );
                return;
            }
        }

        // Check for recursive call patterns in bash/sh
        let prog = parsed.program_basename.as_str();
        if (prog == "bash" || prog == "sh" || prog == "zsh")
            && (raw.contains("(){") && raw.contains("|") && raw.contains("&"))
        {
            detection.add_category(
                DangerCategory::ForkBomb,
                "potential fork bomb pattern in shell command",
            );
            detection.is_dangerous = true;
            detection.severity = 10;
            detection.context_mitigatable = false;
        }

        // Check for while true infinite loops with spawning
        if (raw.contains("while true") || raw.contains("while :") || raw.contains("while 1"))
            && raw.contains("&")
            && (raw.contains("fork") || raw.contains("|"))
        {
            detection.add_category(
                DangerCategory::ForkBomb,
                "potential infinite loop with process spawning",
            );
            detection.is_dangerous = true;
            detection.severity = 9;
            detection.context_mitigatable = true;
        }
    }

    /// Check for remote code execution patterns.
    pub fn check_remote_code_execution(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();
        let raw = &parsed.raw;

        // Check for curl/wget piped to shell
        let downloaders = ["curl", "wget", "fetch", "httpie", "http", "aria2c"];
        let shells = ["sh", "bash", "zsh", "ksh", "fish", "dash", "ash"];

        if downloaders.contains(&prog) && parsed.has_pipe {
            // Check if piped to shell
            for shell in shells {
                if raw.contains(&format!("| {shell}"))
                    || raw.contains(&format!("|{shell}"))
                    || raw.contains(&format!("| /bin/{shell}"))
                    || raw.contains(&format!("|/bin/{shell}"))
                {
                    *detection = DangerDetection::dangerous(
                        DangerCategory::RemoteCodeExecution,
                        format!("{prog} output piped to {shell} - remote code execution"),
                        10,
                        false,
                    );
                    return;
                }
            }

            // Check for python/perl/ruby execution
            let interpreters = ["python", "python3", "perl", "ruby", "node", "php"];
            for interp in interpreters {
                if raw.contains(&format!("| {interp}")) || raw.contains(&format!("|{interp}")) {
                    *detection = DangerDetection::dangerous(
                        DangerCategory::RemoteCodeExecution,
                        format!("{prog} output piped to {interp} - remote code execution"),
                        10,
                        false,
                    );
                    return;
                }
            }
        }

        // Check for eval with remote content
        if (prog == "eval" || (shells.contains(&prog) && parsed.has_arg("-c")))
            && (raw.contains("curl") || raw.contains("wget") || raw.contains("$("))
        {
            detection.add_category(
                DangerCategory::RemoteCodeExecution,
                "eval or shell -c with potential remote content",
            );
            detection.is_dangerous = true;
            detection.severity = 9;
            detection.context_mitigatable = true;
        }

        // Check for xargs with shell commands
        if prog == "xargs" && (parsed.has_arg("sh") || parsed.has_arg("bash")) {
            detection.add_category(
                DangerCategory::RemoteCodeExecution,
                "xargs executing shell commands",
            );
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }
    }

    /// Check for insecure permission settings.
    pub fn check_insecure_permissions(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();

        if prog == "chmod" {
            // Check for overly permissive modes
            let dangerous_modes = ["777", "666", "o+w", "a+w", "+w"];

            for mode in dangerous_modes {
                if parsed.has_arg(mode) {
                    // Check if on sensitive paths
                    for arg in &parsed.args {
                        if arg != mode && !arg.starts_with('-') {
                            let normalized = Self::normalize_path(arg);
                            if self.is_sensitive_path(&normalized) {
                                *detection = DangerDetection::dangerous(
                                    DangerCategory::InsecurePermissions,
                                    format!("chmod {mode} on sensitive path: {arg}"),
                                    9,
                                    false,
                                );
                                return;
                            }
                        }
                    }

                    detection.add_category(
                        DangerCategory::InsecurePermissions,
                        &format!("insecure permission mode: {mode}"),
                    );
                    detection.is_dangerous = true;
                    detection.severity = 6;
                    detection.context_mitigatable = true;
                    break;
                }
            }

            // Check for recursive chmod on root-level directories
            if parsed.has_flag(Some('R'), Some("recursive")) {
                for arg in &parsed.args {
                    if !arg.starts_with('-') {
                        let normalized = Self::normalize_path(arg);
                        if normalized == "/" || self.is_sensitive_path(&normalized) {
                            *detection = DangerDetection::dangerous(
                                DangerCategory::InsecurePermissions,
                                format!("recursive chmod on sensitive path: {arg}"),
                                10,
                                false,
                            );
                            return;
                        }
                    }
                }
            }
        }

        // Check for chown on sensitive paths
        if (prog == "chown" || prog == "chgrp") && parsed.has_flag(Some('R'), Some("recursive")) {
            for arg in &parsed.args {
                if !arg.starts_with('-') && !arg.contains(':') {
                    let normalized = Self::normalize_path(arg);
                    if self.is_sensitive_path(&normalized) {
                        *detection = DangerDetection::dangerous(
                            DangerCategory::InsecurePermissions,
                            format!("recursive {prog} on sensitive path: {arg}"),
                            9,
                            false,
                        );
                        return;
                    }
                }
            }
        }

        // Check for umask with insecure values
        if prog == "umask" && (parsed.has_arg("000") || parsed.has_arg("0000")) {
            detection.add_category(
                DangerCategory::InsecurePermissions,
                "umask set to fully permissive",
            );
            detection.is_dangerous = true;
            detection.severity = 5;
            detection.context_mitigatable = true;
        }
    }

    /// Check for system service modifications.
    pub fn check_system_service_mods(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();

        let service_commands = [
            "systemctl",
            "service",
            "chkconfig",
            "update-rc.d",
            "rc-update",
            "launchctl",
            "sc",  // Windows service control
            "net", // Windows net start/stop
        ];

        if service_commands.contains(&prog) {
            // Check for modification operations
            let dangerous_ops = [
                "stop",
                "start",
                "restart",
                "enable",
                "disable",
                "mask",
                "unmask",
                "daemon-reload",
                "kill",
                "halt",
                "poweroff",
                "reboot",
            ];

            for op in dangerous_ops {
                if parsed.has_arg(op) && !self.config.allow_service_modifications {
                    *detection = DangerDetection::dangerous(
                        DangerCategory::SystemServiceMod,
                        format!("{prog} {op} - system service modification"),
                        8,
                        true,
                    );
                    return;
                }
            }
        }

        // Check for init system commands
        let init_commands = ["init", "telinit", "shutdown", "halt", "reboot", "poweroff"];

        if init_commands.contains(&prog) {
            *detection = DangerDetection::dangerous(
                DangerCategory::SystemServiceMod,
                format!("system state change via {prog}"),
                10,
                false,
            );
        }
    }

    /// Check for network exposure.
    pub fn check_network_exposure(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let prog = parsed.program_basename.as_str();

        // Netcat listening
        if (prog == "nc" || prog == "netcat" || prog == "ncat")
            && parsed.has_flag(Some('l'), Some("listen"))
        {
            let port = parsed.get_flag_value(Some('p'), Some("port"));
            if let Some(p) = port
                && let Ok(port_num) = p.parse::<u16>()
                && self.config.dangerous_ports.contains(&port_num)
            {
                *detection = DangerDetection::dangerous(
                    DangerCategory::NetworkExposure,
                    format!("nc listening on sensitive port {port_num}"),
                    9,
                    true,
                );
                return;
            }
            detection.add_category(
                DangerCategory::NetworkExposure,
                "netcat listening for connections",
            );
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }

        // Python HTTP server
        if (prog == "python" || prog == "python3" || prog == "python2")
            && parsed.has_arg("-m")
            && (parsed.has_arg("http.server")
                || parsed.has_arg("SimpleHTTPServer")
                || parsed.has_arg("CGIHTTPServer"))
        {
            detection.add_category(
                DangerCategory::NetworkExposure,
                "Python HTTP server exposing files",
            );
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }

        // PHP built-in server
        if prog == "php" && parsed.has_flag(Some('S'), None) {
            detection.add_category(
                DangerCategory::NetworkExposure,
                "PHP built-in server exposing files",
            );
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }

        // Ruby HTTP server
        if prog == "ruby" && (parsed.has_arg("-run") || parsed.has_arg_containing("httpd")) {
            detection.add_category(
                DangerCategory::NetworkExposure,
                "Ruby HTTP server exposing files",
            );
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }

        // SSH tunneling
        if prog == "ssh" && (parsed.has_flag(Some('R'), None) || parsed.has_flag(Some('L'), None)) {
            detection.add_category(DangerCategory::NetworkExposure, "SSH tunnel creation");
            detection.is_dangerous = true;
            detection.severity = 6;
            detection.context_mitigatable = true;
        }

        // socat listening
        if prog == "socat" && parsed.raw.to_lowercase().contains("listen") {
            detection.add_category(DangerCategory::NetworkExposure, "socat listening mode");
            detection.is_dangerous = true;
            detection.severity = 7;
            detection.context_mitigatable = true;
        }

        // ngrok/localtunnel exposure
        let tunnel_tools = ["ngrok", "localtunnel", "lt", "serveo", "cloudflared"];
        if tunnel_tools.contains(&prog) {
            detection.add_category(
                DangerCategory::NetworkExposure,
                &format!("tunnel/exposure tool: {prog}"),
            );
            detection.is_dangerous = true;
            detection.severity = 8;
            detection.context_mitigatable = true;
        }
    }

    /// Check for credential/secret access.
    pub fn check_credential_access(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let prog = parsed.program_basename.as_str();

        // Reading sensitive files
        let read_commands = [
            "cat", "less", "more", "head", "tail", "vim", "vi", "nano", "grep",
        ];
        let sensitive_files = [
            "/etc/shadow",
            "/etc/passwd",
            "/etc/sudoers",
            ".ssh/id_rsa",
            ".ssh/id_ed25519",
            ".ssh/id_ecdsa",
            ".gnupg/",
            ".aws/credentials",
            ".kube/config",
            ".docker/config.json",
            ".netrc",
            ".npmrc",
            ".pypirc",
        ];

        if read_commands.contains(&prog) {
            for arg in &parsed.args {
                for sensitive in sensitive_files {
                    if arg.contains(sensitive) {
                        detection.add_category(
                            DangerCategory::CredentialAccess,
                            &format!("reading sensitive file: {arg}"),
                        );
                        detection.is_dangerous = true;
                        detection.severity = 8;
                        detection.context_mitigatable = true;
                        return;
                    }
                }
            }
        }

        // Password/secret dumping tools
        let secret_tools = [
            "unshadow",
            "john",
            "hashcat",
            "mimikatz",
            "secretsdump",
            "lazagne",
            "pass",
            "gpg",
            "keychain",
        ];

        if secret_tools.contains(&prog) {
            detection.add_category(
                DangerCategory::CredentialAccess,
                &format!("credential/secret tool: {prog}"),
            );
            detection.is_dangerous = true;
            detection.severity = 8;
            detection.context_mitigatable = true;
        }

        // Environment variable exposure with secrets
        if prog == "env" || prog == "printenv" || prog == "export" {
            // These are informational but could expose secrets in logs
            detection.add_category(
                DangerCategory::CredentialAccess,
                "environment variable exposure",
            );
            detection.severity = 3;
            // Not dangerous by itself, just a note
        }
    }

    /// Check for kernel/system modifications.
    pub fn check_kernel_modifications(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();

        let kernel_commands = [
            "insmod",
            "rmmod",
            "modprobe",
            "depmod",
            "sysctl",
            "kmod",
            "update-initramfs",
            "dracut",
            "mkinitcpio",
            "grub-install",
            "grub-mkconfig",
            "efibootmgr",
            "bcdedit", // Windows boot config
        ];

        if kernel_commands.contains(&prog) {
            *detection = DangerDetection::dangerous(
                DangerCategory::KernelModification,
                format!("kernel/boot modification via {prog}"),
                10,
                false,
            );
        }

        // Check for direct writes to /proc or /sys
        for arg in &parsed.args {
            if arg.starts_with("/proc/") || arg.starts_with("/sys/") {
                detection.add_category(
                    DangerCategory::KernelModification,
                    &format!("direct access to kernel interface: {arg}"),
                );
                detection.is_dangerous = true;
                detection.severity = 9;
                detection.context_mitigatable = false;
                break;
            }
        }
    }

    /// Check for container escape attempts.
    pub fn check_container_escape(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let prog = parsed.program_basename.as_str();
        let raw = &parsed.raw;

        // Docker socket access
        if raw.contains("/var/run/docker.sock") || raw.contains("docker.sock") {
            detection.add_category(
                DangerCategory::ContainerEscape,
                "Docker socket access (potential container escape)",
            );
            detection.is_dangerous = true;
            detection.severity = 10;
            detection.context_mitigatable = false;
        }

        // Mount namespace manipulation
        if prog == "nsenter" || prog == "unshare" {
            detection.add_category(
                DangerCategory::ContainerEscape,
                &format!("namespace manipulation via {prog}"),
            );
            detection.is_dangerous = true;
            detection.severity = 9;
            detection.context_mitigatable = false;
        }

        // Privileged container operations
        if (prog == "docker" || prog == "podman")
            && (parsed.has_arg("--privileged")
                || parsed.has_arg("--cap-add")
                || parsed.has_arg("--pid=host")
                || parsed.has_arg("--network=host"))
        {
            detection.add_category(
                DangerCategory::ContainerEscape,
                "privileged container operation",
            );
            detection.is_dangerous = true;
            detection.severity = 9;
            detection.context_mitigatable = false;
        }

        // cgroup manipulation
        if raw.contains("/sys/fs/cgroup") {
            detection.add_category(
                DangerCategory::ContainerEscape,
                "cgroup manipulation (potential container escape)",
            );
            detection.is_dangerous = true;
            detection.severity = 9;
            detection.context_mitigatable = false;
        }
    }

    /// Check for history manipulation.
    pub fn check_history_manipulation(
        &self,
        parsed: &ParsedCommand,
        detection: &mut DangerDetection,
    ) {
        let prog = parsed.program_basename.as_str();
        let raw = &parsed.raw;

        // History clearing commands
        if prog == "history" && (parsed.has_arg("-c") || parsed.has_arg("-w")) {
            detection.add_category(
                DangerCategory::HistoryManipulation,
                "shell history manipulation",
            );
            detection.is_dangerous = true;
            detection.severity = 5;
            detection.context_mitigatable = true;
        }

        // Direct history file manipulation
        let history_files = [
            ".bash_history",
            ".zsh_history",
            ".history",
            ".sh_history",
            "fish_history",
        ];

        for hist in history_files {
            if raw.contains(hist) {
                detection.add_category(
                    DangerCategory::HistoryManipulation,
                    &format!("history file access: {hist}"),
                );
                detection.is_dangerous = true;
                detection.severity = 5;
                detection.context_mitigatable = true;
                break;
            }
        }

        // HISTFILE unsetting
        if raw.contains("unset HISTFILE")
            || raw.contains("HISTFILE=/dev/null")
            || raw.contains("HISTSIZE=0")
        {
            detection.add_category(
                DangerCategory::HistoryManipulation,
                "disabling command history",
            );
            detection.is_dangerous = true;
            detection.severity = 6;
            detection.context_mitigatable = true;
        }
    }

    /// Check for potential crypto mining.
    pub fn check_crypto_mining(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        let prog = parsed.program_basename.as_str();
        let raw_lower = parsed.raw.to_lowercase();

        // Known mining software
        let miners = [
            "xmrig",
            "cpuminer",
            "cgminer",
            "bfgminer",
            "ethminer",
            "claymore",
            "phoenixminer",
            "t-rex",
            "gminer",
            "nbminer",
            "minerd",
            "minergate",
        ];

        if miners.contains(&prog) {
            *detection = DangerDetection::dangerous(
                DangerCategory::CryptoMining,
                format!("cryptocurrency miner: {prog}"),
                10,
                false,
            );
            return;
        }

        // Mining pool connections
        let mining_indicators = [
            "stratum+tcp://",
            "stratum+ssl://",
            "pool.mining",
            "nicehash",
            "ethermine",
            "f2pool",
            "nanopool",
            "moneroocean",
            "--algo",
            "--donate-level",
        ];

        for indicator in mining_indicators {
            if raw_lower.contains(indicator) {
                detection.add_category(
                    DangerCategory::CryptoMining,
                    &format!("mining indicator detected: {indicator}"),
                );
                detection.is_dangerous = true;
                detection.severity = 9;
                detection.context_mitigatable = false;
                return;
            }
        }
    }

    /// Check custom dangerous patterns from configuration.
    pub fn check_custom_patterns(&self, parsed: &ParsedCommand, detection: &mut DangerDetection) {
        // Check custom safe patterns first (allow override)
        for pattern in &self.config.custom_safe_patterns {
            if parsed.raw.contains(pattern) || parsed.program_basename == *pattern {
                return; // Explicitly safe
            }
        }

        // Check custom dangerous patterns
        for pattern in &self.config.custom_dangerous_patterns {
            if parsed.raw.contains(pattern) || parsed.program_basename == *pattern {
                detection.add_category(
                    DangerCategory::CustomRule,
                    &format!("matches custom dangerous pattern: {pattern}"),
                );
                detection.is_dangerous = true;
                detection.severity = 8;
                detection.context_mitigatable = true;
                return;
            }
        }
    }
}
