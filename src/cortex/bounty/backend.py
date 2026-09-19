"""Read the external public feed; never substitute local adjudications."""

import json
from collections import Counter
from typing import Literal

import httpx
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from cortex.protocol.crypto import decode_hotkey

from .scoring import BountyScore, Holdout, judge_challenger

Severity = Literal["trivial", "minor", "major", "critical"]
Verdict = Literal["valid", "invalid_malicious", "duplicate", "already_fixed_not_prod"]


class BackendUnavailable(Exception):
    """A public scoring snapshot cannot be trusted. Contains no upstream body."""


class FeedModel(BaseModel):
    model_config = ConfigDict(extra="ignore", frozen=True, strict=True)


class LeaderboardRow(FeedModel):
    hotkey: str
    valid_count: int = Field(ge=0, le=2**64 - 1)
    weight: int | None = Field(default=None, ge=0, le=2**64 - 1)


class PublicReport(FeedModel):
    id: str = Field(min_length=1)
    hotkey: str
    status: Literal["valid", "invalid_malicious", "duplicate", "already_fixed_not_prod", "pending"]
    problem_found: str
    adjudicator: str
    justification: str
    severity: Severity | None = None
    adjudicated_at: str
    created_at: str
    related_report_id: str | None = None


class PublicSnapshot(FeedModel):
    leaderboard: tuple[LeaderboardRow, ...] = ()
    reports: tuple[PublicReport, ...] = ()

    def validate_publication(self) -> None:
        """A stable pair can still be two permanently different revisions."""
        try:
            valid_counts = Counter(
                decode_hotkey(r.hotkey).hex() for r in self.reports if r.status == "valid"
            )
            leaderboard = {decode_hotkey(r.hotkey).hex(): r.valid_count for r in self.leaderboard}
            for report in self.reports:
                decode_hotkey(report.hotkey)
        except ValueError:
            raise BackendUnavailable("backend public invalid hotkey") from None
        if len(leaderboard) != len(self.leaderboard):
            raise BackendUnavailable("backend public duplicate leaderboard hotkey")
        if len({row.id for row in self.reports}) != len(self.reports):
            raise BackendUnavailable("backend public duplicate report id")
        if any(leaderboard.get(k) != count for k, count in valid_counts.items()) or any(
            valid_counts.get(k, 0) != count for k, count in leaderboard.items()
        ):
            raise BackendUnavailable("backend public leaderboard and reports do not agree")

    def score(self, expected: list[str]) -> dict[str, BountyScore]:
        self.validate_publication()
        holdouts: dict[str, Holdout] = {}
        for report in self.reports:
            if (
                report.status == "pending"
                or not report.problem_found.strip()
                or not report.justification.strip()
            ):
                continue
            hotkey = decode_hotkey(report.hotkey).hex()
            holdouts.setdefault(hotkey, Holdout()).record(report.status, report.severity)
        ranked = sorted(self.leaderboard, key=lambda row: (-row.valid_count, row.hotkey))
        order = [decode_hotkey(row.hotkey).hex() for row in ranked]
        already_ranked = set(order)
        order.extend(key for key in sorted(holdouts) if key not in already_ranked)
        champion, champion_holdout, lattice = None, Holdout(), 0
        for hotkey in order:
            if hotkey not in holdouts:
                continue
            verdict = judge_challenger(champion_holdout, holdouts[hotkey])
            if verdict.eligible:
                champion, champion_holdout, lattice = hotkey, holdouts[hotkey], verdict.lattice
        scores = {}
        for raw in expected:
            key = decode_hotkey(raw).hex()
            if key == champion:
                scores[key] = BountyScore(value=lattice)
            else:
                reason = (
                    "InvalidResponse"
                    if key in holdouts and holdouts[key].net_credit < 0
                    else "NotAttempted"
                )
                scores[key] = BountyScore(reason=reason)
        return scores


class PublicBackend:
    """Bounded consecutive reads, parsed DTO equality and revision validation."""

    def __init__(self, base_url: str | None, *, transport: httpx.AsyncBaseTransport | None = None):
        self.base_url = (base_url or "").strip().rstrip("/")
        self.transport = transport

    @property
    def configured(self) -> bool:
        return bool(self.base_url)

    async def fetch(self) -> PublicSnapshot:
        if not self.configured:
            raise BackendUnavailable("scoring unconfigured: set BOUNTY_BACKEND_PUBLIC_URL")
        previous = None
        try:
            async with httpx.AsyncClient(
                timeout=20,
                transport=self.transport,
                follow_redirects=False,
                headers={"User-Agent": "cortex-bounty-challenge/python"},
            ) as client:
                for _ in range(4):
                    leaderboard, lb_token = await self._read(client, "leaderboard", LeaderboardRow)
                    reports, rp_token = await self._read(client, "reports", PublicReport)
                    snapshot = PublicSnapshot(
                        leaderboard=tuple(leaderboard), reports=tuple(reports)
                    )
                    current = (snapshot, lb_token, rp_token)
                    if current == previous:
                        snapshot.validate_publication()
                        if lb_token is not None and rp_token is not None and lb_token != rp_token:
                            raise BackendUnavailable(
                                "backend public publication tokens do not agree"
                            )
                        return snapshot
                    previous = current
        except (httpx.HTTPError, ValueError, ValidationError):
            raise BackendUnavailable("backend public fetch or JSON validation failed") from None
        raise BackendUnavailable("backend public feed changed under every read")

    async def _read(self, client, route, model):
        async with client.stream("GET", f"{self.base_url}/v1/bounty/public/{route}") as response:
            if not 200 <= response.status_code < 300:
                raise BackendUnavailable(
                    f"backend public fetch failed: HTTP {response.status_code}"
                )
            content = bytearray()
            async for part in response.aiter_bytes():
                if len(content) + len(part) > 8 * 1024 * 1024:
                    raise BackendUnavailable("backend public response too large")
                content.extend(part)
            document = json.loads(content)
            token = response.headers.get("etag") or None
            if isinstance(document, dict):
                for key in ("revision", "snapshot_id", "etag"):
                    if isinstance(document.get(key), str) and document[key].strip():
                        token = document[key].strip()
                        break
                rows = document.get("items")
            else:
                rows = document
            if not isinstance(rows, list):
                raise BackendUnavailable("backend public JSON must contain a report list")
            return [model.model_validate(row) for row in rows], token
