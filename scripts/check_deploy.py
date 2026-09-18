"""Validate deployment contracts without starting containers or contacting a host."""

from __future__ import annotations

import argparse
import ipaddress
import json
import math
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
IMAGE = re.compile(r"[a-z0-9][a-z0-9._:/-]*@sha256:[0-9a-f]{64}")
PYTHON_IMAGE = re.compile(
    r"(?:docker.io/library/)?python:3\.12(?:\.[0-9]+)?-slim-bookworm@sha256:[0-9a-f]{64}"
)
HEX_DIGEST = re.compile(r"[0-9a-f]{64}")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}")
ENV_NAME = re.compile(r"[A-Z][A-Z0-9_]*")
CHAIN_ALIASES = frozenset({"archive", "finney", "local", "test"})
VM_HOST_EXEC = "/opt/base/venv/bin/cortex vm-host --config /etc/proof-vm/host.toml"
VM_RESOURCE_ENV = {
    "PROOF_RLM_VM_VCPUS": (1, 16),
    "PROOF_RLM_VM_MEM_MIB": (128, 32768),
    "PROOF_RLM_VM_DISK_MIB": (16384, 1048576),
}
MASTER_HEALTHCHECK = [
    "CMD",
    "python",
    "-c",
    "import urllib.request; urllib.request.urlopen("
    "'http://127.0.0.1:8080/readyz', timeout=5).read()",
]
VALIDATOR_HEALTHCHECK = [
    "CMD",
    "python",
    "-I",
    "-c",
    "import ssl,urllib.request; urllib.request.urlopen("
    "'https://127.0.0.1:8091/livez', context=ssl._create_unverified_context(), "
    "timeout=5).read()",
]

MASTER_ENVIRONMENT = {
    "BASE_STATE_DIR": "/var/lib/base",
    "BASE_OWNER_PUBKEY_FILE": "/etc/base/config/owner.pubkey",
    "BASE_CHALLENGES_FILE": "/etc/base/config/challenges.toml",
    "BASE_MEASUREMENTS_FILE": "/etc/base/config/measurements.toml",
    "BASE_GATEWAY_SK_FILE": "/run/secrets/gateway.key",
    "BOUNTY_SK_FILE": "/run/secrets/bounty.key",
    "PROOF_SK_FILE": "/run/secrets/proof.key",
    "BOUNTY_SESSION_SECRET_FILE": "/run/secrets/bounty-session.key",
    "BASE_GATEWAY_ADMIN_TOKEN_FILE": "/run/secrets/operator.token",
}

VALIDATOR_OPTIONS = {
    "--gateway",
    "--netuid",
    "--gateway-public",
    "--network",
    "--fallback-endpoints",
    "--owner-public",
    "--challenges",
    "--measurements",
    "--minimum-challenges-version",
    "--minimum-measurements-version",
    "--wallet-name",
    "--wallet-hotkey",
    "--wallet-path",
    "--state-db",
    "--consensus-seed-file",
    "--peers",
    "--peer-bind",
    "--peer-port",
    "--peer-tls-certificate",
    "--peer-tls-key",
    "--poll-seconds",
    "--version-key",
    "--min-peer-sample",
    "--max-block-lag",
}


def validate_pin(image: str, *, base: bool = False) -> None:
    if not (PYTHON_IMAGE if base else IMAGE).fullmatch(image):
        raise ValueError("an exact repository@sha256 image pin is required")


def _mapping(value, label: str) -> dict:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a table")
    return value


def _keys(table: dict, required: set[str], allowed: set[str], label: str) -> None:
    missing = required - set(table)
    extra = set(table) - allowed
    if missing or extra:
        raise ValueError(f"{label} has missing or unknown fields")


def _absolute(value, label: str) -> Path:
    if (
        not isinstance(value, str)
        or not value
        or not Path(value).is_absolute()
        or ".." in Path(value).parts
    ):
        raise ValueError(f"{label} must be an absolute path")
    return Path(value)


def _under(path: Path, parent: Path, label: str) -> None:
    if not path.is_relative_to(parent):
        raise ValueError(f"{label} is outside its dedicated deployment directory")


def _integer(value, minimum: int, maximum: int, label: str) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{label} is outside its supported range")
    return value


def validate_vm_resource_environment(environment: dict, *, required: bool = True) -> None:
    if not required and not VM_RESOURCE_ENV.keys() & environment.keys():
        return
    for name, (minimum, maximum) in VM_RESOURCE_ENV.items():
        value = environment.get(name)
        if not isinstance(value, str) or not re.fullmatch(r"[0-9]{1,7}", value):
            raise ValueError(f"VM resource {name} requires an explicit integer")
        _integer(int(value), minimum, maximum, f"VM resource {name}")


def _compose_options(command) -> dict[str, str]:
    if not isinstance(command, list) or not command or command[0] != "validator":
        raise ValueError("validator must invoke the Python validator entrypoint")
    if len(command) % 2 != 1 or not all(isinstance(item, str) for item in command):
        raise ValueError("validator command must contain flag/value pairs")
    options: dict[str, str] = {}
    for index in range(1, len(command), 2):
        flag, value = command[index], command[index + 1]
        if not flag.startswith("--") or not value or flag in options:
            raise ValueError("validator command contains an invalid or duplicate flag")
        options[flag] = value
    if set(options) != VALIDATOR_OPTIONS:
        raise ValueError("validator command differs from the audited Python runtime contract")
    return options


def _wss_endpoints(value: str, label: str) -> list[str]:
    try:
        endpoints = json.loads(value)
    except json.JSONDecodeError as error:
        raise ValueError(f"{label} must be a JSON list") from error
    if (
        not isinstance(endpoints, list)
        or len(endpoints) > 8
        or any(not isinstance(endpoint, str) for endpoint in endpoints)
        or len(set(endpoints)) != len(endpoints)
    ):
        raise ValueError(f"{label} must contain up to 8 unique WSS URLs")
    for endpoint in endpoints:
        try:
            parsed = urlsplit(endpoint)
            port = parsed.port
        except ValueError:
            raise ValueError(f"{label} must contain up to 8 unique WSS URLs") from None
        if (
            endpoint != endpoint.strip()
            or parsed.scheme != "wss"
            or not parsed.hostname
            or parsed.username
            or parsed.password
            or parsed.path not in {"", "/"}
            or parsed.query
            or parsed.fragment
            or port == 0
        ):
            raise ValueError(f"{label} must contain up to 8 unique WSS URLs")
    return endpoints


def _chain_endpoint(value: str, label: str) -> str:
    if value in CHAIN_ALIASES:
        return value
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError:
        raise ValueError(f"{label} must be an official alias or WSS origin") from None
    if (
        not value
        or len(value) > 2048
        or not value.isascii()
        or any(ord(character) < 33 or ord(character) == 127 for character in value)
        or parsed.scheme != "wss"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.path not in {"", "/"}
        or parsed.query
        or parsed.fragment
        or port == 0
    ):
        raise ValueError(f"{label} must be an official alias or WSS origin")
    return value


def _explicit_host_port(service: dict, target: int) -> None:
    ports = service.get("ports", [])
    if not isinstance(ports, list) or len(ports) != 1 or not isinstance(ports[0], dict):
        raise ValueError("role must publish exactly one port")
    port = ports[0]
    host = port.get("host_ip")
    if port.get("target") != target or host in {None, "", "0.0.0.0", "::"}:
        raise ValueError("role port must bind to its explicit host address")


def _mounts(service: dict) -> dict[str, dict]:
    volumes = service.get("volumes", [])
    if not isinstance(volumes, list):
        raise ValueError("volumes must be a list")
    by_target: dict[str, dict] = {}
    for mount in volumes:
        if not isinstance(mount, dict) or not isinstance(mount.get("target"), str):
            raise ValueError("volumes must use long syntax")
        target = mount["target"]
        if target in by_target:
            raise ValueError("duplicate mount target")
        by_target[target] = mount
        if mount.get("type") == "bind":
            if mount.get("read_only") is not True:
                raise ValueError("operator files must be mounted read-only")
            bind = mount.get("bind")
            if isinstance(bind, dict) and bind.get("create_host_path") is not False:
                raise ValueError("operator bind paths must already exist")
        if "docker.sock" in str(mount.get("source", "")):
            raise ValueError("application roles must not control Docker")
    state = by_target.get("/var/lib/base", {})
    if state.get("type") != "volume" or state.get("read_only", False):
        raise ValueError("durable writable role state is required")
    return by_target


def _validate_common_service(service: dict) -> dict[str, dict]:
    validate_pin(service.get("image", ""))
    if (
        service.get("init") is not True
        or service.get("read_only") is not True
        or service.get("restart") != "unless-stopped"
        or service.get("privileged", False)
    ):
        raise ValueError("application containers require a read-only supervised runtime")
    if service.get("entrypoint") not in (None, [], ()):
        raise ValueError("Compose must use the image's audited cortex entrypoint")
    if service.get("user") != "65532:65532" or set(service.get("cap_drop", [])) != {"ALL"}:
        raise ValueError("application containers must drop root and Linux capabilities")
    if "no-new-privileges:true" not in service.get("security_opt", []):
        raise ValueError("privilege escalation must be disabled")
    for field in ("cap_add", "devices"):
        if service.get(field):
            raise ValueError("application containers may not add host privileges")
    for field in ("network_mode", "pid", "ipc"):
        if service.get(field) == "host":
            raise ValueError("application containers may not join host namespaces")
    tmpfs = service.get("tmpfs", [])
    if not isinstance(tmpfs, list) or not any(str(item).startswith("/tmp:") for item in tmpfs):
        raise ValueError("read-only application containers require a bounded /tmp tmpfs")
    return _mounts(service)


def validate_compose(config: dict, role: str) -> None:
    expected = "gateway" if role == "master" else "validator"
    services = _mapping(config.get("services"), "services")
    if set(services) != {expected}:
        raise ValueError(f"{role} role must contain only its {expected} service")
    service = _mapping(services[expected], expected)
    mounts = _validate_common_service(service)
    command = service.get("command", [])

    if role == "master":
        if command != ["master", "--bind", "0.0.0.0", "--port", "8080"]:
            raise ValueError("master command differs from the Python master entrypoint")
        if service.get("profiles") != ["master"]:
            raise ValueError("gateway requires the master profile")
        environment = _mapping(service.get("environment"), "master environment")
        validate_vm_resource_environment(environment, required=False)
        if any(environment.get(key) != value for key, value in MASTER_ENVIRONMENT.items()):
            raise ValueError("master file credentials must use the audited container paths")
        forbidden_values = {
            "OPENROUTER_API_KEY",
            "BASE_GATEWAY_SK",
            "BOUNTY_SK",
            "PROOF_SK",
            "BASE_GATEWAY_ADMIN_TOKEN",
            "BOUNTY_SESSION_SECRET",
            "PROOF_VM_ORCHESTRATOR_TOKEN",
        }
        if forbidden_values.intersection(environment):
            raise ValueError("master credentials must be provided through private files")
        if not {"/etc/base/config", "/run/secrets", "/var/lib/base"} <= set(mounts):
            raise ValueError("master trust, secret and state mounts are required")
        if mounts["/etc/base/config"].get("type") != "bind":
            raise ValueError("master trust root must be an operator bind mount")
        if mounts["/run/secrets"].get("type") != "bind":
            raise ValueError("master credentials must be an operator bind mount")
        if "/run/wallets" in mounts:
            raise ValueError("master must not hold the validator wallet")
        health = service.get("healthcheck", {}).get("test", [])
        if health != MASTER_HEALTHCHECK:
            raise ValueError("master healthcheck must use its Python readiness endpoint")
        _explicit_host_port(service, 8080)
        return

    if service.get("profiles"):
        raise ValueError("validator must not require the master profile")
    if service.get("environment"):
        raise ValueError("validator configuration must be explicit command arguments")
    if not {"/etc/base/config", "/run/wallets", "/run/validator", "/var/lib/base"} <= set(mounts):
        raise ValueError("validator trust, wallet, identity and state mounts are required")
    if "/run/secrets" in mounts:
        raise ValueError("validator must not hold challenge signing keys")
    options = _compose_options(command)
    _chain_endpoint(options["--network"], "validator chain endpoint")
    _wss_endpoints(options["--fallback-endpoints"], "validator fallback endpoints")
    gateway = urlsplit(options["--gateway"])
    if (
        gateway.scheme != "https"
        or not gateway.hostname
        or gateway.username
        or gateway.password
        or gateway.path not in {"", "/"}
        or gateway.query
        or gateway.fragment
    ):
        raise ValueError("validator gateway must be an HTTPS origin without credentials")
    try:
        netuid = int(options["--netuid"])
        poll = float(options["--poll-seconds"])
        challenge_version = int(options["--minimum-challenges-version"])
        measurement_version = int(options["--minimum-measurements-version"])
        version_key = int(options["--version-key"])
        min_peer_sample = int(options["--min-peer-sample"])
        max_block_lag = int(options["--max-block-lag"])
    except ValueError:
        raise ValueError("validator numeric options are invalid") from None
    if (
        not 0 <= netuid <= 65535
        or not math.isfinite(poll)
        or poll <= 0
        or challenge_version < 1
        or measurement_version < 1
        or not 0 <= version_key <= 2**64 - 1
        or not 0 <= min_peer_sample <= 64
        or max_block_lag < 1
    ):
        raise ValueError("validator numeric options are invalid")
    fixed = {
        "--owner-public": "/etc/base/config/owner.pubkey",
        "--challenges": "/etc/base/config/challenges.toml",
        "--measurements": "/etc/base/config/measurements.toml",
        "--wallet-path": "/run/wallets",
        "--state-db": "/var/lib/base/validator.sqlite3",
        "--consensus-seed-file": "/run/validator/consensus.key",
        "--peers": "/etc/base/config/peers.json",
        "--peer-bind": "0.0.0.0",
        "--peer-port": "8091",
        "--peer-tls-certificate": "/run/validator/tls.crt",
        "--peer-tls-key": "/run/validator/tls.key",
    }
    if any(options[key] != value for key, value in fixed.items()):
        raise ValueError("validator paths or peer listener differ from the runtime contract")
    if not all(
        options[name]
        for name in ("--gateway-public", "--network", "--wallet-name", "--wallet-hotkey")
    ):
        raise ValueError("validator identity and network pins are required")
    health = service.get("healthcheck", {}).get("test", [])
    if health != VALIDATOR_HEALTHCHECK:
        raise ValueError("validator healthcheck must use its Python liveness endpoint")
    _explicit_host_port(service, 8091)


def _vm_host_shape(config: dict, *, placeholders: bool) -> None:
    required_sections = {"host", "tls", "inference", "images", "caps", "knowledge"}
    allowed_sections = required_sections | {"limits", "egress"}
    _keys(config, required_sections, allowed_sections, "VM host configuration")
    host = _mapping(config["host"], "host")
    host_fields = {
        "bind",
        "port",
        "token_file",
        "kernel",
        "kernel_digest",
        "jail_root",
        "retain_root",
        "pack_dir",
        "state_db",
        "firecracker",
        "jailer",
        "uid",
        "gid",
        "uplink",
        "max_experiments",
        "max_topics",
        "custom_ids",
    }
    _keys(host, host_fields, host_fields, "host")
    try:
        bind = ipaddress.ip_address(host["bind"])
    except ValueError:
        raise ValueError("VM host bind must be one explicit IP address") from None
    if bind.is_unspecified or bind.is_multicast:
        raise ValueError("VM host may not bind every interface")
    _integer(host["port"], 1, 65535, "VM host port")
    _integer(host["uid"], 1, 2**31 - 1, "jailer uid")
    _integer(host["gid"], 1, 2**31 - 1, "jailer gid")
    _integer(host["max_experiments"], 1, 128, "experiment capacity")
    _integer(host["max_topics"], 1, 1024, "topic capacity")
    if not isinstance(host["uplink"], str) or not re.fullmatch(
        r"[A-Za-z0-9_.-]{1,15}", host["uplink"]
    ):
        raise ValueError("invalid VM host uplink")
    paths = {
        field: _absolute(host[field], f"host.{field}")
        for field in (
            "token_file",
            "kernel",
            "jail_root",
            "retain_root",
            "pack_dir",
            "state_db",
            "firecracker",
            "jailer",
        )
    }
    _under(paths["token_file"], Path("/etc/proof-vm"), "host token")
    _under(paths["kernel"], Path("/var/lib/proof/images"), "kernel")
    for field in ("jail_root", "retain_root", "pack_dir", "state_db"):
        _under(paths[field], Path("/var/lib/proof"), f"host.{field}")
    if paths["firecracker"] != Path("/usr/local/bin/firecracker") or paths["jailer"] != Path(
        "/usr/local/bin/jailer"
    ):
        raise ValueError("Firecracker and jailer must use the systemd read-only paths")
    custom_ids = host["custom_ids"]
    if (
        not isinstance(custom_ids, list)
        or len(custom_ids) > 128
        or len(custom_ids) != len(set(custom_ids))
        or any(not isinstance(item, str) or not IDENTIFIER.fullmatch(item) for item in custom_ids)
    ):
        raise ValueError("invalid custom runner registry")

    tls = _mapping(config["tls"], "tls")
    tls_fields = {"certificate", "private_key", "names"}
    _keys(tls, tls_fields, tls_fields, "tls")
    for field in ("certificate", "private_key"):
        _under(_absolute(tls[field], f"tls.{field}"), Path("/etc/proof-vm"), f"tls.{field}")
    names = tls["names"]
    if (
        not isinstance(names, list)
        or len(names) != len(set(names))
        or any(not isinstance(item, str) or not item for item in names)
    ):
        raise ValueError("TLS SAN names must be strings")
    if not placeholders and not names:
        raise ValueError("TLS SAN names must pin the control-plane host")

    inference = _mapping(config["inference"], "inference")
    inference_required = {"api_key_file", "offer_file", "offer_commitment"}
    _keys(inference, inference_required, inference_required | {"model"}, "inference")
    for field in ("api_key_file", "offer_file"):
        _under(
            _absolute(inference[field], f"inference.{field}"),
            Path("/etc/proof-vm"),
            f"inference.{field}",
        )
    model = inference.get("model")
    if model is not None and (
        not isinstance(model, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}", model)
    ):
        raise ValueError("invalid inference model identifier")

    images = _mapping(config["images"], "images")
    for digest, value in images.items():
        if not isinstance(digest, str) or not HEX_DIGEST.fullmatch(digest):
            raise ValueError("rootfs images require sha256 keys")
        _under(_absolute(value, f"images.{digest}"), Path("/var/lib/proof/images"), "rootfs")
    if len(set(images.values())) != len(images):
        raise ValueError("each rootfs digest must name its own file")

    caps = _mapping(config["caps"], "caps")
    cap_fields = {"vcpus", "mem_mib", "disk_mib"}
    _keys(caps, cap_fields, cap_fields, "caps")
    _integer(caps["vcpus"], 1, 16, "vCPU cap")
    _integer(caps["mem_mib"], 128, 32768, "memory cap")
    _integer(caps["disk_mib"], 16384, 1048576, "disk cap")

    knowledge = _mapping(config["knowledge"], "knowledge")
    knowledge_fields = {"state_db", "owner_public_file"}
    _keys(knowledge, knowledge_fields, knowledge_fields, "knowledge")
    _under(
        _absolute(knowledge["state_db"], "knowledge.state_db"),
        Path("/var/lib/proof"),
        "knowledge database",
    )
    _under(
        _absolute(knowledge["owner_public_file"], "knowledge.owner_public_file"),
        Path("/etc/proof-vm"),
        "knowledge owner public key",
    )

    egress = config.get("egress", [])
    if not isinstance(egress, list) or len(egress) > 64:
        raise ValueError("egress must be a bounded list")
    for row in egress:
        rule = _mapping(row, "egress rule")
        _keys(rule, {"cidr", "port"}, {"cidr", "port", "protocol"}, "egress rule")
        if not isinstance(rule["cidr"], str):
            raise ValueError("invalid topic egress network")
        try:
            network = ipaddress.ip_network(rule["cidr"], strict=False)
        except ValueError:
            raise ValueError("invalid topic egress network") from None
        if network.version != 4:
            raise ValueError("topic egress is IPv4-only")
        _integer(rule["port"], 1, 65535, "egress port")
        if rule.get("protocol", "tcp") not in {"tcp", "udp"}:
            raise ValueError("invalid egress protocol")

    limits = config.get("limits")
    if limits is not None:
        values = _mapping(limits, "limits")
        integer_limits = {
            "max_calls": (1, 128),
            "max_tool_calls": (1, 512),
            "max_tokens": (256, 2_000_000),
            "max_depth": (0, 8),
            "completion_tokens": (64, 32_768),
            "max_response_bytes": (1024, 1_048_576),
            "context_bytes": (8192, 262_144),
            "compact_keep_exchanges": (0, 16),
        }
        float_limits = {"wall_seconds", "tool_timeout_seconds"}
        allowed_limits = set(integer_limits) | float_limits
        _keys(values, set(), allowed_limits, "limits")
        for name, (minimum, maximum) in integer_limits.items():
            if name in values:
                _integer(values[name], minimum, maximum, f"limits.{name}")
        for name in float_limits.intersection(values):
            value = values[name]
            if (
                isinstance(value, bool)
                or not isinstance(value, int | float)
                or not math.isfinite(value)
                or not 0 < value <= 7200
            ):
                raise ValueError(f"limits.{name} is outside its supported range")

    if placeholders:
        return
    if not HEX_DIGEST.fullmatch(str(host["kernel_digest"])):
        raise ValueError("kernel requires an exact sha256 pin")
    if not HEX_DIGEST.fullmatch(str(inference["offer_commitment"])):
        raise ValueError("signed inference offer commitment is required")
    if not images:
        raise ValueError("at least one digest-pinned guest rootfs is required")


def validate_vm_host(config: dict) -> None:
    _vm_host_shape(config, placeholders=False)


def validate_vm_host_template(config: dict) -> None:
    _vm_host_shape(config, placeholders=True)
    host = config["host"]
    inference = config["inference"]
    if (
        host["kernel_digest"] != ""
        or config["images"] != {}
        or inference["offer_commitment"] != ""
        or config["tls"]["names"] != []
        or host["custom_ids"] != []
    ):
        raise ValueError("checked-in VM host template must not invent live operator pins")


def validate_dockerfile(source: str) -> None:
    match = re.search(r"(?m)^ARG PYTHON_IMAGE=(\S+)$", source)
    if match is None:
        raise ValueError("Dockerfile must declare its pinned Python base")
    validate_pin(match.group(1), base=True)
    required = (
        "FROM ${PYTHON_IMAGE} AS builder",
        "FROM ${PYTHON_IMAGE} AS guest",
        "FROM ${PYTHON_IMAGE} AS runtime",
        "COPY pyproject.toml uv.lock ./",
        "COPY src ./src",
        "uv sync --locked",
        "--require-hashes",
        "proof-guest-init",
        'ENTRYPOINT ["/sbin/init"]',
        "USER 65532:65532",
        'ENTRYPOINT ["cortex"]',
        "READ_ONLY=1",
    )
    if any(item not in source for item in required):
        raise ValueError("Dockerfile differs from the audited Python runtime/guest stages")
    stages = re.findall(r"(?m)^FROM\s+(\S+)\s+AS\s+(\S+)\s*$", source)
    if stages != [
        ("${PYTHON_IMAGE}", "builder"),
        ("builder", "guest-builder"),
        ("builder", "runtime-builder"),
        ("${PYTHON_IMAGE}", "guest"),
        ("${PYTHON_IMAGE}", "runtime"),
    ]:
        raise ValueError("Dockerfile may contain only the audited Python build stages")
    runtime = source.split("FROM ${PYTHON_IMAGE} AS runtime", 1)[1]
    instructions = (
        line.split("#", 1)[0].strip() for line in runtime.replace("\\\n", " ").splitlines()
    )
    installs = (
        match.group(1).split()
        for instruction in instructions
        if instruction.startswith("RUN ")
        for match in re.finditer(r"\bapt-get\s+install\s+([^;&|]+)", instruction)
    )
    if not any("openssh-client" in packages for packages in installs):
        raise ValueError("Dockerfile runtime must install openssh-client for provider transport")
    copies = [line.strip() for line in source.splitlines() if line.startswith("COPY ")]
    if copies != [
        "COPY pyproject.toml uv.lock ./",
        "COPY src ./src",
        "COPY --from=guest-builder /opt/cortex /opt/cortex",
        "COPY --from=runtime-builder /opt/cortex /opt/cortex",
    ]:
        raise ValueError("Dockerfile may copy only manifests, Python source and the built venv")
    if re.search(r"(?m)^COPY\s+\.\s", source):
        raise ValueError("Dockerfile may not copy the whole repository into an image")
    if not re.search(r"pip install --no-cache-dir uv==[0-9]+\.[0-9]+\.[0-9]+", source):
        raise ValueError("Docker build frontend must use an exact uv version")


def validate_rootfs_builder(source: str) -> None:
    required = (
        "--target guest",
        "--iidfile",
        "src/cortex/vm/image.py",
        "--source-date-epoch",
        "sha256-$DIGEST.ext4",
    )
    if any(marker not in source for marker in required):
        raise ValueError(
            "rootfs builder does not bind the Python guest stage to measured ext4 bytes"
        )
    if any(marker in source for marker in ("cargo", "proof-vm-guest-agent", "target/release")):
        raise ValueError("rootfs builder still depends on the removed Rust guest")


def _systemd(source: str) -> dict[str, dict[str, list[str]]]:
    sections: dict[str, dict[str, list[str]]] = {}
    current: dict[str, list[str]] | None = None
    for raw in source.splitlines():
        line = raw.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            current = sections.setdefault(line[1:-1], {})
            continue
        if current is None or "=" not in line:
            raise ValueError("invalid systemd unit syntax")
        key, value = line.split("=", 1)
        current.setdefault(key, []).append(value)
    return sections


def validate_systemd(source: str) -> None:
    unit = _systemd(source)
    service = unit.get("Service", {})

    def one(name: str) -> str:
        values = service.get(name, [])
        if len(values) != 1:
            raise ValueError(f"systemd service requires one {name}")
        return values[0]

    if unit.get("Unit", {}).get("ConditionPathExists") != ["/dev/kvm"]:
        raise ValueError("VM host must refuse to start without /dev/kvm")
    expected = {
        "Type": "simple",
        "User": "root",
        "UMask": "0077",
        "WorkingDirectory": "/var/lib/proof",
        "ExecStart": VM_HOST_EXEC,
        "Restart": "on-failure",
        "KillMode": "control-group",
        "ProtectSystem": "strict",
        "ProtectHome": "yes",
        "PrivateTmp": "yes",
        "ProtectKernelModules": "yes",
        "ProtectKernelLogs": "yes",
        "ProtectHostname": "yes",
        "ProtectClock": "yes",
        "NoNewPrivileges": "yes",
        "ProtectKernelTunables": "no",
        "ProtectControlGroups": "no",
        "RestrictAddressFamilies": "AF_UNIX AF_INET AF_INET6 AF_NETLINK",
        "SystemCallArchitectures": "native",
        "AmbientCapabilities": "",
        "StateDirectory": "proof",
        "StateDirectoryMode": "0700",
        "Delegate": "yes",
        "RestartSec": "5",
        "TimeoutStopSec": "7500",
        "LimitNOFILE": "65536",
    }
    if any(one(key) != value for key, value in expected.items()):
        raise ValueError("VM host systemd isolation differs from the audited contract")
    writable = set(one("ReadWritePaths").split())
    if not {"/var/lib/proof", "/run"} <= writable:
        raise ValueError("VM host writable paths are incomplete")
    read_only = set(one("ReadOnlyPaths").split())
    if (
        not {
            "/etc/proof-vm",
            "/opt/base/venv",
            "/usr/local/bin/firecracker",
            "/usr/local/bin/jailer",
        }
        <= read_only
    ):
        raise ValueError("VM host executable and credential paths must be read-only")
    required_caps = {
        "CAP_SYS_ADMIN",
        "CAP_SYS_CHROOT",
        "CAP_MKNOD",
        "CAP_NET_ADMIN",
        "CAP_SETUID",
        "CAP_SETGID",
        "CAP_CHOWN",
        "CAP_DAC_OVERRIDE",
        "CAP_FOWNER",
        "CAP_KILL",
        "CAP_SYS_RESOURCE",
    }
    if set(one("CapabilityBoundingSet").split()) != required_caps:
        raise ValueError("VM host capabilities differ from the audited jailer/network set")


def _env_file(source: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw in source.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        name, separator, value = line.partition("=")
        if not separator or not ENV_NAME.fullmatch(name) or name in values:
            raise ValueError("invalid or duplicate environment example field")
        if "`" in value or "$(" in value:
            raise ValueError("environment examples may not execute shell expressions")
        values[name] = value
    return values


def validate_env_examples(master_source: str, validator_source: str) -> None:
    master = _env_file(master_source)
    validate_vm_resource_environment(master)
    master_required = {
        "CORTEX_IMAGE",
        "BASE_GATEWAY_BIND_ADDRESS",
        "BASE_NETUID",
        "BASE_CHAIN_ENDPOINT",
        "BASE_CHAIN_FALLBACK_ENDPOINTS",
        "BASE_EMIT_POLL_SECS",
        "BASE_EPOCH_REFRESH_SECS",
        "BASE_EPOCH_STALE_SECS",
        "BASE_CHALLENGES_MIN_VERSION",
        "BASE_MEASUREMENTS_MIN_VERSION",
        "BOUNTY_BACKEND_PUBLIC_URL",
        "PROOF_VM_ORCHESTRATOR_URL",
        "PROOF_VM_ORCHESTRATOR_TOKEN_FILE",
        "PROOF_VM_ORCHESTRATOR_CA_FILE",
        "PROOF_RLM_VM_IMAGE_DIGEST",
        "PROOF_INFERENCE_OFFER_COMMITMENT",
        "PROOF_VM_RUNNER_CUSTOM_IDS",
        *VM_RESOURCE_ENV,
    }
    if set(master) != master_required:
        raise ValueError("master environment example differs from Python runtime settings")
    intentionally_unset = {
        "CORTEX_IMAGE",
        "BASE_GATEWAY_BIND_ADDRESS",
        "BOUNTY_BACKEND_PUBLIC_URL",
        "PROOF_VM_ORCHESTRATOR_URL",
        "PROOF_RLM_VM_IMAGE_DIGEST",
        "PROOF_INFERENCE_OFFER_COMMITMENT",
        "PROOF_VM_RUNNER_CUSTOM_IDS",
    }
    if any(master[name] for name in intentionally_unset):
        raise ValueError("master example must not invent image, backend or Proof pins")
    if master["PROOF_VM_ORCHESTRATOR_TOKEN_FILE"] != "/run/secrets/proof-vm.token":
        raise ValueError("master orchestrator bearer must remain a mounted file")
    if master["PROOF_VM_ORCHESTRATOR_CA_FILE"] != "/run/secrets/proof-vm-ca.pem":
        raise ValueError("master orchestrator CA must remain a mounted file")
    try:
        netuid = int(master["BASE_NETUID"])
        emit = float(master["BASE_EMIT_POLL_SECS"])
        refresh = float(master["BASE_EPOCH_REFRESH_SECS"])
        stale = float(master["BASE_EPOCH_STALE_SECS"])
        challenge_version = int(master["BASE_CHALLENGES_MIN_VERSION"])
        measurement_version = int(master["BASE_MEASUREMENTS_MIN_VERSION"])
    except ValueError:
        raise ValueError("master example contains invalid numeric settings") from None
    if (
        not 0 <= netuid <= 65535
        or not all(math.isfinite(item) for item in (emit, refresh, stale))
        or min(emit, refresh) <= 0
        or stale <= refresh
        or challenge_version < 1
        or measurement_version < 1
    ):
        raise ValueError("master example contains unsafe timing or netuid settings")
    _wss_endpoints(master["BASE_CHAIN_FALLBACK_ENDPOINTS"], "master fallback endpoints")
    _chain_endpoint(master["BASE_CHAIN_ENDPOINT"], "master chain endpoint")

    validator = _env_file(validator_source)
    validator_required = {
        "CORTEX_IMAGE",
        "BASE_GATEWAY_ENDPOINT",
        "BASE_NETUID",
        "BASE_CHAIN_ENDPOINT",
        "BASE_CHAIN_FALLBACK_ENDPOINTS",
        "BASE_GATEWAY_HOTKEY",
        "BASE_WALLET_NAME",
        "BASE_WALLET_HOTKEY",
        "BASE_COORDINATION_INTERVAL_SECS",
        "BASE_CHALLENGES_MIN_VERSION",
        "BASE_MEASUREMENTS_MIN_VERSION",
        "BASE_VERSION_KEY",
        "BASE_MIN_PEER_SAMPLE",
        "BASE_MAX_BLOCK_LAG",
        "BASE_VALIDATOR_WALLETS_DIR",
        "BASE_TRUST_ROOT_DIR",
        "BASE_VALIDATOR_IDENTITY_DIR",
        "BASE_VALIDATOR_BIND_ADDRESS",
    }
    if set(validator) != validator_required:
        raise ValueError("validator environment example differs from Python runtime settings")
    for name in validator_required - {
        "BASE_NETUID",
        "BASE_CHAIN_ENDPOINT",
        "BASE_CHAIN_FALLBACK_ENDPOINTS",
        "BASE_COORDINATION_INTERVAL_SECS",
        "BASE_MIN_PEER_SAMPLE",
        "BASE_MAX_BLOCK_LAG",
    }:
        if validator[name]:
            raise ValueError("validator host-specific identity fields must remain blank")
    try:
        validator_netuid = int(validator["BASE_NETUID"])
        validator_poll = float(validator["BASE_COORDINATION_INTERVAL_SECS"])
        validator_min_peer_sample = int(validator["BASE_MIN_PEER_SAMPLE"])
        validator_max_block_lag = int(validator["BASE_MAX_BLOCK_LAG"])
    except ValueError:
        raise ValueError("validator example contains invalid numeric settings") from None
    if (
        not 0 <= validator_netuid <= 65535
        or not math.isfinite(validator_poll)
        or validator_poll <= 0
        or not 0 <= validator_min_peer_sample <= 64
        or validator_max_block_lag < 1
    ):
        raise ValueError("validator example contains unsafe network settings")
    _chain_endpoint(validator["BASE_CHAIN_ENDPOINT"], "validator chain endpoint")
    _wss_endpoints(validator["BASE_CHAIN_FALLBACK_ENDPOINTS"], "validator fallback endpoints")


def _default_sources(config: dict, role: str) -> None:
    service_name = "gateway" if role == "master" else "validator"
    service = config["services"][service_name]
    mounts = {item["target"]: Path(item["source"]) for item in service["volumes"]}
    expected = {"/etc/base/config": ROOT / "config"}
    if role == "master":
        expected["/run/secrets"] = ROOT / "deploy/secrets/master"
        env_files = service.get("env_file", [])
        if len(env_files) != 1 or Path(env_files[0]["path"]) != ROOT / "deploy/env/master.env":
            raise ValueError("master default env file resolves outside the repository deploy tree")
    else:
        expected["/run/wallets"] = ROOT / "deploy/secrets/wallets"
    for target, source in expected.items():
        if mounts.get(target) != source:
            raise ValueError(f"default {role} mount {target} resolves outside the repository")


def check_examples() -> None:
    validate_dockerfile((ROOT / "deploy/Dockerfile").read_text())
    validate_rootfs_builder((ROOT / "deploy/guest/bake-rootfs.sh").read_text())
    validate_systemd((ROOT / "deploy/systemd/proof-vm-orchestrator.service").read_text())
    validate_env_examples(
        (ROOT / "deploy/env/master.env.example").read_text(),
        (ROOT / "deploy/env/python-validator.env.example").read_text(),
    )
    validate_vm_resource_environment(_env_file((ROOT / ".env.example").read_text()))
    with (ROOT / "deploy/env/proof-vm-host.toml.example").open("rb") as stream:
        validate_vm_host_template(tomllib.load(stream))

    removed = {
        "BASE_MASTER_ENV_FILE",
        "BASE_MASTER_SECRETS_DIR",
        "BASE_TRUST_ROOT_DIR",
        "BASE_VALIDATOR_WALLETS_DIR",
    }
    env = {key: value for key, value in os.environ.items() if key not in removed}
    env.update(
        {
            "CORTEX_IMAGE": "fixture.invalid/cortex@sha256:" + "f" * 64,
            "BASE_GATEWAY_BIND_ADDRESS": "127.0.0.1",
            "BASE_GATEWAY_ENDPOINT": "https://master.fixture.invalid",
            "BASE_NETUID": "541",
            "BASE_CHAIN_ENDPOINT": "test",
            "BASE_GATEWAY_HOTKEY": "e" * 64,
            "BASE_WALLET_NAME": "fixture",
            "BASE_WALLET_HOTKEY": "validator",
            "BASE_CHALLENGES_MIN_VERSION": "1",
            "BASE_MEASUREMENTS_MIN_VERSION": "1",
            "BASE_VERSION_KEY": "1",
            "BASE_VALIDATOR_BIND_ADDRESS": "127.0.0.2",
            "BASE_VALIDATOR_IDENTITY_DIR": "/fixture/identity",
        }
    )
    for role in ("master", "validator"):
        result = subprocess.run(
            [
                "docker",
                "compose",
                "--profile",
                "*",
                "--project-directory",
                str(ROOT),
                "-f",
                str(ROOT / "deploy/compose" / f"role-{role}.yml"),
                "config",
                "--no-env-resolution",
                "--format",
                "json",
            ],
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
        config = json.loads(result.stdout)
        validate_compose(config, role)
        _default_sources(config, role)
    print("Python images, roles, VM host and fail-closed pins validated; no services started.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--base-image")
    modes.add_argument("--check-examples", action="store_true")
    modes.add_argument("--config", type=Path, help="rendered Compose JSON; use - for stdin")
    modes.add_argument("--vm-host-config", type=Path, help="materialized Proof host TOML")
    parser.add_argument("--role", choices=["master", "validator"])
    args = parser.parse_args()
    try:
        if args.base_image:
            validate_pin(args.base_image, base=True)
        elif args.check_examples:
            check_examples()
        elif args.config and args.role:
            source = sys.stdin.read() if str(args.config) == "-" else args.config.read_text()
            validate_compose(json.loads(source), args.role)
        elif args.vm_host_config and not args.role:
            with args.vm_host_config.open("rb") as stream:
                validate_vm_host(tomllib.load(stream))
        else:
            parser.error("--config requires --role; --vm-host-config forbids it")
    except ValueError as error:
        raise SystemExit(f"Deployment check failed: {error}") from None
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Deployment check failed: {type(error).__name__}") from None


if __name__ == "__main__":
    main()
