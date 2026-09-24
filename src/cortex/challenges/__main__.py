"""`cortex challenge-supervisor`: the only Cortex process that controls Docker."""

from __future__ import annotations

import argparse
import asyncio
import logging
from pathlib import Path

import httpx

from .supervisor import Supervisor, SupervisorConfig


def parser() -> argparse.ArgumentParser:
    arguments = argparse.ArgumentParser(description="Run and auto-update challenge containers")
    arguments.add_argument("--registry", type=Path, required=True)
    arguments.add_argument(
        "--secrets-host-dir",
        type=Path,
        required=True,
        help="absolute HOST path holding <challenge id>/{internal.token,admin.token,...}",
    )
    arguments.add_argument("--docker-socket", default="/var/run/docker.sock")
    arguments.add_argument("--network", default="cortex-challenges")
    arguments.add_argument("--master-url", default="http://cortex-master:8080")
    arguments.add_argument("--once", action="store_true", help="reconcile every entry once")
    return arguments


async def run(arguments: argparse.Namespace) -> None:
    config = SupervisorConfig(
        registry_file=arguments.registry,
        secrets_host_dir=arguments.secrets_host_dir,
        network=arguments.network,
        master_url=arguments.master_url,
    )
    transport = httpx.AsyncHTTPTransport(uds=arguments.docker_socket)
    async with (
        httpx.AsyncClient(transport=transport, base_url="http://docker/v1.44") as docker,
        httpx.AsyncClient(trust_env=False, follow_redirects=False) as http,
    ):
        supervisor = Supervisor(config, docker, http)
        if arguments.once:
            await supervisor.tick(float("inf"))
            return
        await supervisor.run()


def main(argv: list[str] | None = None) -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    try:
        asyncio.run(run(parser().parse_args(argv)))
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
