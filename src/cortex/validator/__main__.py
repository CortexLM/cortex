"""Run with ``python -m cortex.validator``; requires the ``chain`` package extra."""

import argparse
import asyncio
import json
import logging
from math import isfinite
from pathlib import Path
from urllib.parse import urlsplit

import httpx
import uvicorn

from cortex.config import read_seed, validate_chain_endpoint
from cortex.protocol import ProtocolError
from cortex.protocol.crypto import decode_hotkey, public_key
from cortex.protocol.trust import load_trust_root

from .chain import BittensorChain, close_subtensor
from .evidence import peer_app
from .service import SubmissionJournal, TickResult, Validator


def fallback_endpoints(value: str) -> list[str]:
    try:
        endpoints = json.loads(value)
    except json.JSONDecodeError as error:
        raise argparse.ArgumentTypeError("fallback endpoints must be a JSON list") from error
    if (
        not isinstance(endpoints, list)
        or len(endpoints) > 8
        or any(not isinstance(endpoint, str) for endpoint in endpoints)
        or len(set(endpoints)) != len(endpoints)
    ):
        raise argparse.ArgumentTypeError("fallback endpoints must be up to 8 unique WSS URLs")
    for endpoint in endpoints:
        try:
            parsed = urlsplit(endpoint)
            port = parsed.port
        except ValueError:
            raise argparse.ArgumentTypeError(
                "fallback endpoints must be up to 8 unique WSS URLs"
            ) from None
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
            raise argparse.ArgumentTypeError("fallback endpoints must be up to 8 unique WSS URLs")
    return endpoints


def primary_endpoint(value: str) -> str:
    try:
        return validate_chain_endpoint(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError(str(error)) from None


def parser() -> argparse.ArgumentParser:
    arguments = argparse.ArgumentParser(
        description="Verify Cortex seals and submit Bittensor weights"
    )
    arguments.add_argument("--gateway", required=True)
    arguments.add_argument("--netuid", required=True, type=int)
    arguments.add_argument("--gateway-public", required=True, help="independently pinned hotkey")
    arguments.add_argument("--owner-public", type=Path, default=Path("config/owner.pubkey"))
    arguments.add_argument("--challenges", type=Path, default=Path("config/challenges.toml"))
    arguments.add_argument("--measurements", type=Path, default=Path("config/measurements.toml"))
    arguments.add_argument("--minimum-challenges-version", type=int, required=True)
    arguments.add_argument("--minimum-measurements-version", type=int, required=True)
    arguments.add_argument("--network", type=primary_endpoint, default="finney")
    arguments.add_argument("--fallback-endpoints", type=fallback_endpoints, default=[])
    arguments.add_argument("--wallet-name", required=True)
    arguments.add_argument("--wallet-hotkey", required=True)
    arguments.add_argument("--wallet-path", default="~/.bittensor/wallets")
    arguments.add_argument("--state-db", type=Path, required=True)
    arguments.add_argument("--poll-seconds", type=float, default=30)
    arguments.add_argument("--version-key", type=int, required=True)
    arguments.add_argument("--once", action="store_true")
    arguments.add_argument(
        "--consensus-seed-file",
        type=Path,
        required=True,
        help="private sr25519 seed for the same validator hotkey",
    )
    arguments.add_argument(
        "--peers", type=Path, help="JSON mapping independent validator hotkeys to HTTPS origins"
    )
    arguments.add_argument(
        "--peer-consensus",
        action="store_true",
        help="require a peer-root sample before submitting; off by default because the "
        "master gateway is authoritative for weights and this validator consumes "
        "/v1/weights/latest. Opt in only for independently-operated multi-validator "
        "deployments, where --peers and --min-peer-sample then apply",
    )
    arguments.add_argument("--min-peer-sample", type=int, default=1)
    arguments.add_argument("--max-block-lag", type=int, default=256)
    arguments.add_argument(
        "--verify-only",
        action="store_true",
        help="verify current sealed weights without claiming or submitting them",
    )
    arguments.add_argument("--peer-bind", default="127.0.0.1")
    arguments.add_argument("--peer-port", type=int, default=8091)
    arguments.add_argument("--peer-tls-certificate", type=Path)
    arguments.add_argument("--peer-tls-key", type=Path)
    return arguments


async def run(arguments: argparse.Namespace, subtensor, wallet) -> TickResult | None:
    epoch = await asyncio.to_thread(subtensor.get_subnet_epoch_index, arguments.netuid)
    if epoch is None:
        raise ProtocolError("cannot read subnet epoch")

    def load_trust(epoch):
        return load_trust_root(
            challenges_path=arguments.challenges,
            challenges_signature=Path(str(arguments.challenges) + ".sig"),
            measurements_path=arguments.measurements,
            measurements_signature=Path(str(arguments.measurements) + ".sig"),
            owner_public=decode_hotkey(arguments.owner_public.read_text().strip()),
            gateway_public=decode_hotkey(arguments.gateway_public),
            epoch=epoch,
            minimum_challenges_version=arguments.minimum_challenges_version,
            minimum_measurements_version=arguments.minimum_measurements_version,
        )

    def consensus_seed():
        seed = read_seed(arguments.consensus_seed_file)
        if public_key(seed) != wallet.hotkey.public_key:
            raise ProtocolError("consensus seed must match validator wallet hotkey")
        return seed

    consensus_seed()
    peers = {}
    if arguments.peers:
        values = json.loads(arguments.peers.read_text())
        if not isinstance(values, dict):
            raise ProtocolError("peer config must map hotkeys to HTTPS URLs")
        peers = {decode_hotkey(key): value for key, value in values.items()}
    if arguments.peer_bind not in {"127.0.0.1", "::1", "localhost"} and (
        not arguments.peer_tls_certificate or not arguments.peer_tls_key
    ):
        raise ProtocolError("public peer listener requires TLS certificate and key")
    arguments.state_db.parent.mkdir(parents=True, exist_ok=True)
    journal = SubmissionJournal(arguments.state_db)
    peer_server = uvicorn.Server(
        uvicorn.Config(
            peer_app(journal.evidence),
            host=arguments.peer_bind,
            port=arguments.peer_port,
            ssl_certfile=str(arguments.peer_tls_certificate)
            if arguments.peer_tls_certificate
            else None,
            ssl_keyfile=str(arguments.peer_tls_key) if arguments.peer_tls_key else None,
            access_log=False,
            proxy_headers=False,
        )
    )
    peer_task = asyncio.create_task(peer_server.serve())
    try:
        async with httpx.AsyncClient(timeout=30, follow_redirects=False, trust_env=False) as http:
            validator = Validator(
                gateway_url=arguments.gateway,
                netuid=arguments.netuid,
                trust=load_trust(epoch),
                chain=BittensorChain(subtensor, wallet),
                journal=journal,
                http=http,
                version_key=arguments.version_key,
                consensus_seed=consensus_seed,
                peers=peers,
                peer_consensus=arguments.peer_consensus,
                min_peer_sample=arguments.min_peer_sample,
                max_block_lag=arguments.max_block_lag,
                trust_loader=load_trust,
                verify_only=arguments.verify_only,
            )
            while True:
                if peer_task.done():
                    raise ProtocolError("peer evidence server stopped")
                try:
                    result = await validator.run_once()
                    logging.info(
                        "validator outcome=%s epoch=%s attempt_id=%s extrinsic_hash=%s nonce=%s",
                        result.outcome,
                        result.epoch,
                        result.attempt_id,
                        result.extrinsic_hash,
                        result.nonce,
                    )
                except (ProtocolError, httpx.HTTPError, OSError) as error:
                    logging.warning("validator tick refused (%s)", type(error).__name__)
                    if arguments.once:
                        raise
                if arguments.once:
                    return result
                await asyncio.sleep(arguments.poll_seconds)
    finally:
        peer_server.should_exit = True
        await peer_task
        journal.close()


def main(argv: list[str] | None = None) -> None:
    arguments = parser().parse_args(argv)
    if not isfinite(arguments.poll_seconds) or arguments.poll_seconds <= 0:
        raise SystemExit("--poll-seconds must be positive")
    if arguments.minimum_challenges_version < 1 or arguments.minimum_measurements_version < 1:
        raise SystemExit("minimum trust versions must be positive")
    if not 0 <= arguments.netuid <= 65535:
        raise SystemExit("--netuid must fit u16")
    if not 0 <= arguments.version_key <= 2**64 - 1:
        raise SystemExit("--version-key must fit u64")
    if not 0 <= arguments.min_peer_sample <= 64:
        raise SystemExit("--min-peer-sample must be between 0 and 64")
    if arguments.max_block_lag < 1:
        raise SystemExit("--max-block-lag must be positive")
    try:
        from bittensor import Subtensor
        from bittensor_wallet import Wallet
    except ImportError as error:
        raise SystemExit("Install cortex-subnet[chain] to run the validator") from error
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    subtensor = Subtensor(
        network=arguments.network, fallback_endpoints=arguments.fallback_endpoints
    )
    wallet = Wallet(
        name=arguments.wallet_name,
        hotkey=arguments.wallet_hotkey,
        path=str(Path(arguments.wallet_path).expanduser()),
    )
    try:
        result = asyncio.run(run(arguments, subtensor, wallet))
        if (
            arguments.verify_only
            and arguments.once
            and (result is None or result.outcome != "verified")
        ):
            outcome = result.outcome if result is not None else "no_result"
            raise SystemExit(f"validator preflight refused: {outcome}")
    except KeyboardInterrupt:
        pass
    finally:
        close_subtensor(subtensor)


if __name__ == "__main__":
    main()
