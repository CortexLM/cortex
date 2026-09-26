"""Operational entry points; challenge code only starts on the master role."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from pathlib import Path

import httpx
import uvicorn

from cortex.errors import ServiceError
from cortex.http import read_private_file
from cortex.miner import MinerClient, load_seed
from cortex.protocol.crypto import decode_hotkey
from cortex.wallet import HotkeySigner, load_wallet_hotkey


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="cortex", description="Cortex research network")
    commands = root.add_subparsers(dest="command", required=True)
    master = commands.add_parser("master", help="serve the gateway, Proof and challenge proxy")
    master.add_argument("--bind", default="127.0.0.1")
    master.add_argument("--port", type=int, default=8080)
    commands.add_parser("validator", help="verify seals and submit Bittensor weights")
    commands.add_parser(
        "challenge-supervisor", help="run and auto-update registered challenge containers"
    )
    reconcile = commands.add_parser(
        "validator-reconcile", help="resolve one ambiguous validator dispatch"
    )
    reconcile.add_argument("--state-db", type=Path, required=True)
    reconcile.add_argument("--netuid", type=int, required=True)
    reconcile.add_argument("--epoch", type=int, required=True)
    reconcile.add_argument("--digest", required=True)
    reconcile.add_argument("--attempt-id", required=True)
    reconcile.add_argument("--result", choices=("submitted", "not_broadcast"), required=True)
    reconcile.add_argument("--evidence-digest", required=True)
    keygen = commands.add_parser("keygen", help="generate an offline sr25519 operator key")
    keygen.add_argument("--seed-out", type=Path, required=True)
    keygen.add_argument("--public-out", type=Path, required=True)
    trust_sign = commands.add_parser(
        "trust-sign", help="sign a challenge or measurement trust document"
    )
    trust_sign.add_argument("--kind", choices=("challenges", "measurements"), required=True)
    trust_sign.add_argument("--input", type=Path, required=True)
    trust_sign.add_argument("--seed-file", type=Path, required=True)
    trust_sign.add_argument("--signature-out", type=Path, required=True)
    trust_verify = commands.add_parser(
        "trust-verify", help="verify signed trust documents against local pins"
    )
    trust_verify.add_argument("--challenges", type=Path, required=True)
    trust_verify.add_argument("--measurements", type=Path, required=True)
    trust_verify.add_argument("--owner-public", type=Path, required=True)
    trust_verify.add_argument("--gateway-public", required=True)
    trust_verify.add_argument("--epoch", type=int, required=True)
    trust_verify.add_argument("--minimum-challenges-version", type=int, default=1)
    trust_verify.add_argument("--minimum-measurements-version", type=int, default=1)
    vm = commands.add_parser("vm-host", help="serve the dedicated Firecracker host")
    vm.add_argument("--config", type=Path, required=True)
    vm.add_argument(
        "--check", action="store_true", help="check local prerequisites without serving"
    )
    owner = commands.add_parser("topic-create", help="set up and publish a topic through its RLM")
    owner.add_argument("--gateway", required=True)
    owner.add_argument("--token-file", type=Path, required=True)
    owner.add_argument("--policy", type=Path, required=True)
    owner.add_argument("--env-file", type=Path)
    miner = commands.add_parser("miner", help="submit signed research or Bounty reports")
    miner.add_argument("--gateway", required=True)
    identity = miner.add_mutually_exclusive_group(required=True)
    identity.add_argument("--wallet-name", help="Bittensor coldkey wallet name")
    identity.add_argument("--dev-seed-file", type=Path, help="development fixtures only")
    miner.add_argument("--wallet-hotkey", default="default")
    miner.add_argument("--wallet-path", default="~/.bittensor/wallets")
    miner.add_argument("--wallet-password-file", type=Path)
    miner.add_argument("--proof-public", help="pinned Proof owner public key; required for Proof")
    actions = miner.add_subparsers(dest="action", required=True)
    submit = actions.add_parser("proof-submit")
    submit.add_argument("--topic", required=True)
    submit.add_argument("--artifact", type=Path, required=True)
    submit.add_argument("--claim-file", type=Path, required=True)
    submit.add_argument("--manifest", type=Path)
    submit.add_argument("--env-file", type=Path)
    submit.add_argument("--receipt", type=Path, required=True)
    submit.add_argument("--declared-flops", type=int, default=0)
    submit.add_argument(
        "--submit-timeout-secs",
        type=float,
        default=float(os.getenv("CTX_PROOF_SUBMIT_TIMEOUT_SECS", "7200")),
    )
    lookup = actions.add_parser("proof-lookup")
    lookup.add_argument("--receipt", type=Path, required=True)
    pair = actions.add_parser("bounty-pair")
    pair.add_argument("--account-id", required=True)
    pair.add_argument("--accept-terms", action="store_true")
    pair.add_argument("--session-file", type=Path, required=True)
    report = actions.add_parser("bounty-report")
    report.add_argument("--session-file", type=Path, required=True)
    report.add_argument("--title", required=True)
    report.add_argument("--body-file", type=Path, required=True)
    report.add_argument("--repro-file", type=Path, required=True)
    return root


def _miner_env(path: Path | None) -> dict[str, str]:
    if path is None:
        return {}
    value = json.loads(read_private_file(path, 32768))
    if not isinstance(value, dict) or any(
        not isinstance(k, str) or not isinstance(v, str) for k, v in value.items()
    ):
        raise ValueError("env file must contain a JSON object of string values")
    return value


async def run_miner(args: argparse.Namespace) -> None:
    if args.action == "proof-submit" and args.proof_public is None:
        raise ServiceError(400, "--proof-public required for Proof submissions")
    signer = (
        HotkeySigner.from_dev_seed(load_seed(args.dev_seed_file))
        if args.dev_seed_file is not None
        else load_wallet_hotkey(
            name=args.wallet_name,
            hotkey=args.wallet_hotkey,
            path=args.wallet_path,
            password_file=args.wallet_password_file,
        )
    )
    async with httpx.AsyncClient(trust_env=False) as http:
        client = MinerClient(
            base_url=args.gateway,
            signer=signer,
            proof_public_key=decode_hotkey(args.proof_public) if args.proof_public else None,
            client=http,
            submit_timeout_seconds=getattr(args, "submit_timeout_secs", 7200),
        )
        if args.action == "proof-submit":
            result = await client.submit_proof(
                topic_id=args.topic,
                artifact=args.artifact.read_bytes(),
                claim=args.claim_file.read_text(),
                manifest=json.loads(args.manifest.read_text()) if args.manifest else None,
                declared_flops=args.declared_flops,
                env=_miner_env(args.env_file),
                receipt_path=args.receipt,
            )
        elif args.action == "proof-lookup":
            result = await client.lookup_proof(args.receipt)
        elif args.action == "bounty-pair":
            result = await client.pair_bounty(
                account_id=args.account_id, accept_terms=args.accept_terms
            )
            descriptor = os.open(
                args.session_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600
            )
            with os.fdopen(descriptor, "w") as stream:
                stream.write(str(result["session"]))
            result = {"paired": True, "hotkey": client.hotkey}
        else:
            result = await client.report_bounty(
                session=read_private_file(args.session_file),
                title=args.title,
                body=args.body_file.read_text(),
                repro_steps=args.repro_file.read_text(),
            )
        print(json.dumps(result, indent=2))


async def run_topic_create(args: argparse.Namespace) -> None:
    token = read_private_file(args.token_file)
    payload = {"policy": json.loads(args.policy.read_text()), "env": _miner_env(args.env_file)}
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False) as client:
        response = await client.post(
            args.gateway.rstrip("/") + "/challenge/proof/v1/admin/proof/setup",
            headers={"Authorization": "Bearer " + token},
            json=payload,
            timeout=httpx.Timeout(7260, connect=30),
        )
        response.raise_for_status()
        print(json.dumps(response.json(), indent=2))


async def run_master(args: argparse.Namespace) -> None:
    from cortex.config import MasterConfig
    from cortex.master import BittensorEpochProvider, build_master
    from cortex.proof.backend import VmBackend
    from cortex.proof.service import EvaluationBackend, UnwiredBackend
    from cortex.validator.chain import BittensorChain, close_subtensor

    try:
        from bittensor import Subtensor
    except ImportError:
        raise SystemExit("Install cortex-subnet[chain] to run the master") from None
    config = MasterConfig.from_env()
    backend: EvaluationBackend = UnwiredBackend()
    if config.proof_orchestrator_url:
        if config.proof_orchestrator_token_file is None:
            raise ValueError("Proof orchestrator token file required")
        backend = VmBackend(
            url=config.proof_orchestrator_url,
            token_file=config.proof_orchestrator_token_file,
            ca_file=config.proof_orchestrator_ca_file,
            image_digest=os.environ.get("PROOF_RLM_VM_IMAGE_DIGEST", ""),
            inference_offer_commitment=os.environ.get("PROOF_INFERENCE_OFFER_COMMITMENT", ""),
            custom_ids=frozenset(
                filter(None, os.getenv("PROOF_VM_RUNNER_CUSTOM_IDS", "").split(","))
            ),
            resources=config.proof_vm_resources,
        )
    subtensor = Subtensor(
        network=config.chain_endpoint,
        fallback_endpoints=list(config.chain_fallback_endpoints),
    )
    try:
        runtime = await build_master(
            config,
            chain=BittensorChain(subtensor, None),
            epochs=BittensorEpochProvider(subtensor),
            proof_backend=backend,
        )
        await uvicorn.Server(
            uvicorn.Config(runtime.app(), host=args.bind, port=args.port, log_level="info")
        ).serve()
    finally:
        if isinstance(backend, VmBackend):
            await backend.close()
        await asyncio.to_thread(close_subtensor, subtensor)


def run_keygen(args: argparse.Namespace) -> None:
    from cortex.operator import generate_key

    public = generate_key(args.seed_out, args.public_out)
    print(json.dumps({"public_key": public, "seed_file": str(args.seed_out)}))


def run_trust_sign(args: argparse.Namespace) -> None:
    from cortex.operator import sign_trust_document

    sign_trust_document(
        input_path=args.input,
        kind=args.kind,
        seed_path=args.seed_file,
        signature_path=args.signature_out,
    )
    print(json.dumps({"kind": args.kind, "signature_file": str(args.signature_out)}))


def run_trust_verify(args: argparse.Namespace) -> None:
    from cortex.protocol.trust import load_trust_root

    trust = load_trust_root(
        challenges_path=args.challenges,
        challenges_signature=Path(str(args.challenges) + ".sig"),
        measurements_path=args.measurements,
        measurements_signature=Path(str(args.measurements) + ".sig"),
        owner_public=decode_hotkey(args.owner_public.read_text().strip()),
        gateway_public=decode_hotkey(args.gateway_public),
        epoch=args.epoch,
        minimum_challenges_version=args.minimum_challenges_version,
        minimum_measurements_version=args.minimum_measurements_version,
    )
    print(
        json.dumps(
            {
                "challenges": {name.decode(): share for name, share in trust.shares},
                "challenges_version": trust.challenges_version,
                "measurements_version": trust.measurements_version,
                "measurements_digest": trust.measurements_digest.hex(),
            },
            sort_keys=True,
        )
    )


def run_validator_reconcile(args: argparse.Namespace) -> None:
    from cortex.validator import SubmissionJournal

    journal = SubmissionJournal(args.state_db)
    try:
        result = journal.reconcile(
            netuid=args.netuid,
            epoch=args.epoch,
            digest=args.digest,
            attempt_id=args.attempt_id,
            result=args.result,
            evidence_digest=args.evidence_digest,
        )
    finally:
        journal.close()
    print(json.dumps(result, indent=2, sort_keys=True))


def main(argv: list[str] | None = None) -> None:
    values = sys.argv[1:] if argv is None else argv
    if values and values[0] == "validator":
        from cortex.validator.__main__ import main as validator

        validator(values[1:])
        return
    if values and values[0] == "challenge-supervisor":
        from cortex.challenges.__main__ import main as supervisor

        supervisor(values[1:])
        return
    args = parser().parse_args(values)
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    try:
        if args.command == "vm-host":
            if args.check:
                from cortex.vm.preflight import check_host

                report = check_host(args.config)
                print(report.model_dump_json())
                if not report.ready:
                    raise SystemExit(1)
                return
            from cortex.vm.server import serve

            serve(args.config)
        elif args.command == "keygen":
            run_keygen(args)
        elif args.command == "trust-sign":
            run_trust_sign(args)
        elif args.command == "trust-verify":
            run_trust_verify(args)
        elif args.command == "validator-reconcile":
            run_validator_reconcile(args)
        else:
            action = {"master": run_master, "miner": run_miner, "topic-create": run_topic_create}
            asyncio.run(action[args.command](args))
    except (ServiceError, ValueError, OSError, httpx.HTTPError) as error:
        # Request bodies and provider errors can contain credentials; do not dump them.
        reason = error.reason if isinstance(error, ServiceError) else type(error).__name__
        raise SystemExit(f"Cortex command failed: {reason}") from None
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
