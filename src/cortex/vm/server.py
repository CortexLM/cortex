"""Start an authenticated VM host from explicit, non-secret TOML configuration."""

from __future__ import annotations

import tomllib
from pathlib import Path

import uvicorn
from cryptography import x509
from cryptography.x509.oid import ExtensionOID

from cortex.rlm import AgentLimits
from cortex.rlm.knowledge import KnowledgeStore
from cortex.rlm.offer import InferenceOffer, OfferBoundProvider
from cortex.rlm.provider import OpenRouterClient
from cortex.vm.api import create_app
from cortex.vm.firecracker import FirecrackerHypervisor, HostConfig
from cortex.vm.models import Resources
from cortex.vm.network import Egress
from cortex.vm.research import ResearchHost
from cortex.vm.runtime import Orchestrator


def validate_tls_names(certificate: Path, names: list[str]) -> None:
    from ipaddress import ip_address

    if not names:
        raise ValueError("TLS SAN names must include the control-plane orchestrator hostname")
    cert = x509.load_pem_x509_certificate(certificate.read_bytes())
    extension = cert.extensions.get_extension_for_oid(ExtensionOID.SUBJECT_ALTERNATIVE_NAME)
    san = extension.value
    if not isinstance(san, x509.SubjectAlternativeName):
        raise ValueError("TLS certificate lacks SAN")
    dns = san.get_values_for_type(x509.DNSName)
    addresses = san.get_values_for_type(x509.IPAddress)
    for name in names:
        try:
            address = ip_address(name)
        except ValueError:
            if name not in dns:
                raise ValueError("TLS certificate SAN does not match configured hostname") from None
        else:
            if address not in addresses:
                raise ValueError("TLS certificate SAN does not match configured address")


def configured_app(path: Path):
    with path.open("rb") as stream:
        config = tomllib.load(stream)
    host = config["host"]
    tls = config["tls"]
    inference = config["inference"]
    offer = InferenceOffer.load(Path(inference["offer_file"]), inference["offer_commitment"])
    limits = AgentLimits.model_validate(config.get("limits", offer.limits.model_dump()))
    model = inference.get("model", offer.model)
    offer.verify_runtime(model, limits, inference["offer_commitment"])
    validate_tls_names(Path(tls["certificate"]), tls["names"])
    hypervisor = FirecrackerHypervisor(
        HostConfig(
            kernel=Path(host["kernel"]),
            kernel_digest=host["kernel_digest"],
            images={key: Path(value) for key, value in config["images"].items()},
            jail_root=Path(host["jail_root"]),
            retain_root=Path(host["retain_root"]),
            pack_dir=Path(host["pack_dir"]),
            firecracker=Path(host.get("firecracker", "/usr/local/bin/firecracker")),
            jailer=Path(host.get("jailer", "/usr/local/bin/jailer")),
            uid=int(host.get("uid", 10000)),
            gid=int(host.get("gid", 10000)),
            uplink=host.get("uplink", "eth0"),
            topic_egress=tuple(Egress(**row) for row in config.get("egress", [])),
        )
    )
    orchestrator = Orchestrator(
        Path(host["state_db"]),
        hypervisor,
        max_experiments=int(host.get("max_experiments", 1)),
        max_topics=int(host.get("max_topics", 64)),
        caps=Resources.model_validate(config.get("caps", {})),
    )
    provider = OfferBoundProvider(
        OpenRouterClient(model=model, api_key_file=Path(inference["api_key_file"])),
        offer,
        inference["offer_commitment"],
    )
    research = ResearchHost(
        orchestrator,
        provider,
        hypervisor.socket_for,
        limits=limits,
        knowledge=KnowledgeStore(
            Path(config["knowledge"]["state_db"]),
            owner_public_key=bytes.fromhex(
                Path(config["knowledge"]["owner_public_file"]).read_text().strip()
            ),
        )
        if "knowledge" in config
        else None,
    )
    app = create_app(
        orchestrator,
        Path(host["token_file"]),
        research,
        inference_offer_commitment=inference["offer_commitment"],
        inference_offer=offer,
        custom_ids=tuple(host.get("custom_ids", [])),
    )
    return app, {
        "host": host.get("bind", "127.0.0.1"),
        "port": int(host.get("port", 8443)),
        "ssl_certfile": tls["certificate"],
        "ssl_keyfile": tls["private_key"],
        "proxy_headers": False,
    }


def serve(path: Path) -> None:
    app, options = configured_app(path)
    uvicorn.run(app, **options)
