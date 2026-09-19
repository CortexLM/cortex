"""Migration gates protect payment configuration, API promises and repository boundaries."""

import runpy
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
CHECK = runpy.run_path(str(ROOT / "scripts/check_repo.py"))


def put(root, name, content):
    target = root / name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)
    return target


def trust_root(proof_share=8000, third=""):
    return (
        'version = 1\n[[challenges]]\nid = "bounty"\n'
        f'public_key = "{"a" * 64}"\nemission_share_bps = 2000\n'
        '[[challenges]]\nid = "proof"\n'
        f'public_key = "{"b" * 64}"\nemission_share_bps = {proof_share}\n' + third
    )


def test_frozen_spec_single_byte_change_is_detected(tmp_path):
    for name in CHECK["FROZEN_SPECS"]:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    assert CHECK["check_specs"](tmp_path) == []
    changed = tmp_path / "docs/BUNDLE_SPEC.md"
    changed.write_bytes(changed.read_bytes() + b"\n")

    failures = CHECK["check_specs"](tmp_path)

    assert len(failures) == 1
    assert "BUNDLE_SPEC.md" in failures[0] and "SHA-256 changed" in failures[0]


@pytest.mark.parametrize("mutation", ["share", "third", "duplicate", "malformed", "staging"])
def test_trust_root_rejects_emission_drift_or_extra_products(tmp_path, mutation):
    path = put(tmp_path, "config/challenges.toml", trust_root())
    assert CHECK["check_trust_roots"](tmp_path) == []
    if mutation == "share":
        path.write_text(trust_root(proof_share=7000))
    elif mutation == "third":
        path.write_text(trust_root(third='[[challenges]]\nid = "prism"\nemission_share_bps = 0\n'))
    elif mutation == "duplicate":
        path.write_text(trust_root().replace('id = "proof"', 'id = "bounty"'))
    elif mutation == "malformed":
        path.write_text('challenges = "not an array"')
    else:
        put(tmp_path, "config/challenges.staging.toml", trust_root(proof_share=7000))
    assert len(CHECK["check_trust_roots"](tmp_path)) == 1


@pytest.mark.parametrize("method", ["get", "post"])
@pytest.mark.parametrize("path_argument", ['"/v1/status"', 'path="/v1/status"'])
def test_public_api_must_be_an_actual_route_not_a_comment_or_mapping_access(method, path_argument):
    source = (
        '# @router.post("/v1/submissions")\n'
        'mapping.get("/v1/proof/topics")\n'
        f"@router.{method}({path_argument})\nasync def status(): return {{}}\n"
    )
    assert CHECK["declared_routes"](source) == {(method, "/v1/status")}


def test_miner_contract_detects_removed_route_and_undocumented_nonce(tmp_path):
    for name in CHECK["DOC_CONTRACTS"]:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    for name in CHECK["PUBLIC_ROUTES"]:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    proof = tmp_path / "docs/external-miner/proof.md"
    proof.write_text(proof.read_text().replace("submit_nonce", "request_id"))
    api = tmp_path / "src/cortex/proof/api.py"
    api.write_text(api.read_text().replace('"/v1/submissions"', '"/v1/retired"'))

    failures = CHECK["check_public_contracts"](tmp_path)

    assert any("submit_nonce" in error for error in failures)
    assert any("POST /v1/submissions" in error for error in failures)


@pytest.mark.parametrize(
    "name",
    [
        "deploy/env/master.env",
        ".env.production",
        "deploy/secrets/provider.token",
        "src/__pycache__/module.pyc",
        "dist/project.whl",
        "state.sqlite3",
    ],
)
def test_runtime_files_and_caches_cannot_enter_the_tree(tmp_path, name):
    put(tmp_path, name, "placeholder")
    failures = CHECK["check_hygiene"](tmp_path, [Path(name)])
    assert len(failures) == 1


def test_credential_detection_never_prints_the_secret(tmp_path):
    secret = "sk-or-v1-" + "a1" * 32
    put(tmp_path, "example.py", f'api_key = "{secret}"')
    failures = CHECK["check_hygiene"](tmp_path, [Path("example.py")])
    assert failures and "redacted" in failures[0]
    assert secret not in str(failures)


def test_git_inventory_ignores_local_cache_but_catches_force_tracked_cache(tmp_path):
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    put(tmp_path, ".gitignore", "__pycache__/\n")
    put(tmp_path, "src/__pycache__/local.pyc", "local")
    put(tmp_path, "src/__pycache__/tracked.pyc", "tracked")
    subprocess.run(
        ["git", "-C", str(tmp_path), "add", "-f", "src/__pycache__/tracked.pyc"], check=True
    )

    files = CHECK["repository_files"](tmp_path)
    failures = CHECK["check_hygiene"](tmp_path, files)

    assert Path("src/__pycache__/local.pyc") not in files
    assert len(failures) == 1 and "tracked.pyc" in failures[0]


def test_final_gate_removes_code_but_preserves_archived_docs_and_toml_comments(tmp_path):
    names = [
        "src/legacy.rs",
        "Cargo.toml",
        "config/removed.toml",
        "docs/external-miner/prism.md",
        "config/current.toml",
    ]
    for name, body in zip(
        names,
        [
            "fn main() {}",
            '[package]\nname="legacy"',
            'challenge="prism"',
            "Prism is retired",
            '# prism is retired\nchallenge="proof"',
        ],
        strict=True,
    ):
        put(tmp_path, name, body)
    failures = CHECK["check_final"](tmp_path, [Path(name) for name in names])
    assert len(failures) == 3
    assert not any("current.toml" in error or "prism.md" in error for error in failures)


def test_symlink_cannot_hide_a_spec_or_read_a_secret_outside_repository(tmp_path):
    put(tmp_path, "outside", "private")
    root = tmp_path / "repo"
    root.mkdir()
    (root / "link").symlink_to(tmp_path / "outside")
    failures = CHECK["check_hygiene"](root, [Path("link")])
    assert failures == ["link: symlink leaves the repository"]


@pytest.mark.parametrize(
    "name,body",
    [
        ("src/design.py", "value = 1"),
        ("deploy/service.yml", "services:\n  design:\n    image: disabled\n"),
        ("config/choice.yaml", "challenge_id: design\n"),
        ("src/challenge.py", 'challenge_id = "prism"\n'),
    ],
)
def test_final_gate_blocks_removed_products_in_paths_and_configuration(tmp_path, name, body):
    put(tmp_path, name, body)
    assert CHECK["check_final"](tmp_path, [Path(name)])


def test_default_gate_tolerates_rust_until_final_flag_is_enabled(tmp_path):
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    for name in (
        set(CHECK["FROZEN_SPECS"]) | set(CHECK["DOC_CONTRACTS"]) | set(CHECK["PUBLIC_ROUTES"])
    ):
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    put(tmp_path, "config/challenges.toml", trust_root())
    put(tmp_path, "Cargo.toml", '[package]\nname="legacy"')

    assert CHECK["check_repository"](tmp_path) == []
    assert CHECK["check_repository"](tmp_path, final=True) == [
        "Cargo.toml: Rust/Cargo file remains after Python migration"
    ]
