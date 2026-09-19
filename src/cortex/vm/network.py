"""Optional operator-scoped egress for topic setup; experiment guests get no NIC."""

from dataclasses import dataclass
from ipaddress import IPv4Network

from .models import VmError


@dataclass(frozen=True)
class Egress:
    cidr: str
    port: int
    protocol: str = "tcp"

    def __post_init__(self):
        IPv4Network(self.cidr, strict=False)
        if not 1 <= self.port <= 65535 or self.protocol not in {"tcp", "udp"}:
            raise ValueError("invalid topic egress rule")


@dataclass(frozen=True)
class NetworkPlan:
    index: int
    uplink: str
    allow: tuple[Egress, ...]

    @property
    def tap(self) -> str:
        return f"pfc{self.index}"

    @property
    def host(self) -> str:
        return str(IPv4Network("172.30.0.0/16").network_address + self.index * 4 + 1)

    @property
    def guest(self) -> str:
        return str(IPv4Network("172.30.0.0/16").network_address + self.index * 4 + 2)

    def rules(self) -> str:
        name, tap = f"proof_{self.tap}", self.tap
        lines = [
            f"table inet {name} {{",
            "chain forward {",
            "type filter hook forward priority 0; policy accept;",
            f'oifname "{tap}" ct state established,related accept',
            f'oifname "{tap}" drop',
        ]
        for allow in self.allow:
            lines.append(
                f'iifname "{tap}" ip daddr {IPv4Network(allow.cidr, strict=False)} '
                f"{allow.protocol} dport {allow.port} accept"
            )
        lines.extend(
            [
                f'iifname "{tap}" drop',
                "}",
                "chain input {",
                "type filter hook input priority 0; policy accept;",
                f'iifname "{tap}" drop',
                "}",
                "chain postrouting {",
                "type nat hook postrouting priority 100; policy accept;",
                f'ip saddr {self.guest} oifname "{self.uplink}" masquerade',
                "}",
                "}",
            ]
        )
        return "\n".join(lines) + "\n"

    async def up(self, commands, root, uid):
        if not self.allow:
            raise VmError("topic egress allowlist required")
        path = root / "network.nft"
        path.write_text(self.rules())
        # Filter exists before bringing the guest interface up.
        await commands.run(["nft", "-f", str(path)])
        await commands.run(
            ["ip", "tuntap", "add", "dev", self.tap, "mode", "tap", "user", str(uid)]
        )
        await commands.run(["ip", "addr", "add", f"{self.host}/30", "dev", self.tap])
        await commands.run(["ip", "link", "set", self.tap, "up"])
        await commands.run(["sysctl", "-q", "-w", "net.ipv4.ip_forward=1"])

    async def down(self, commands):
        await commands.run(["ip", "link", "del", self.tap])
        await commands.run(["nft", "delete", "table", "inet", f"proof_{self.tap}"])
