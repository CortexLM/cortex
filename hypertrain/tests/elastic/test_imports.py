"""What the elastic package imports from the rest of hypertrain, and who imports it (ADR 0002)."""

import ast
from pathlib import Path

import hypertrain

SRC = Path(hypertrain.__file__).parent

# ADR 0002, Decision 8. `_check_key` and `_verified` are private to data.store; `_check_key` is
# also imported by storage/cas.py, storage/offer.py, data/stream_store.py and, inside a function,
# aggregator/capacity_worker.py, and `_verified` by storage/cas.py.
IMPORTED = {
    "hypertrain.data.store": {"CorruptObjectError", "ObjectNotFound", "_check_key", "_verified"},
    "hypertrain.protocol.envelope": {"EnvelopeError"},
    "hypertrain.protocol.envelope_v2": {"load_json"},
    "hypertrain.protocol.jcs": {"canonicalize"},
    "hypertrain.protocol.keys": {"KeyError_", "Keypair", "decode_hotkey"},
    "hypertrain.protocol.messages": {"Hex64", "SS58"},
    "hypertrain.protocol.messages_v2": {"Identifier", "Positive", "U64", "WireModel"},
    "hypertrain.protocol.server_keys": {"Sig", "check_sig"},
}


def hypertrain_imports(path: Path) -> list[tuple[str, str]]:
    """(module, name) for each `from m import n` and (module, "*") for each `import m`; a
    relative module keeps its leading dots."""
    found: list[tuple[str, str]] = []
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.ImportFrom):
            module = "." * node.level + (node.module or "")
            found += [(module, alias.name) for alias in node.names]
        elif isinstance(node, ast.Import):
            found += [(alias.name, "*") for alias in node.names]
    return [(m, n) for m, n in found if m.startswith((".", "hypertrain.")) or m == "hypertrain"]


def test_the_package_imports_from_the_rest_of_hypertrain_only_what_the_adr_lists() -> None:
    found: dict[str, set[str]] = {}
    for path in (SRC / "elastic").glob("*.py"):
        for module, name in hypertrain_imports(path):
            if "elastic" not in module.split("."):  # the package's own modules
                found.setdefault(module, set()).add(name)
    assert found == IMPORTED


def test_nothing_in_the_rest_of_hypertrain_imports_the_package() -> None:
    importers = [
        str(path.relative_to(SRC))
        for path in sorted(SRC.rglob("*.py"))
        if path.relative_to(SRC).parts[0] != "elastic"
        and any(
            "elastic" in module.split(".") or name == "elastic"
            for module, name in hypertrain_imports(path)
        )
    ]
    assert importers == []
