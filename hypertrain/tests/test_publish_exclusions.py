"""Publisher selector/copy/scanner functions, without cloning, commits or pushes."""

import json
import os
import subprocess
from pathlib import Path

import pytest

# Generated fakes are assembled from short pieces so this file itself passes the scan.
FAKE = "Aa1Bb2Cc3Dd4" + "Ee5Ff6Gg7Hh8Ii9Jj0Kk"
API_KEY_NAME = "api_" + "key"
BEGIN = "-" * 5 + "BEGIN"
DIGIT_FREE = "QwErTyUiOpAsDf" + "GhJkLzXcVbNmQwEr"  # 30 chars, no digit


def test_real_gitleaks_exact_values_and_same_line_new_secrets(tmp_path: Path) -> None:
    # Given: optional local official binary; release gate supplies it explicitly.
    binary = os.environ.get("HYPERTRAIN_GITLEAKS_BIN")
    if binary is None:
        import shutil

        binary = shutil.which("gitleaks")
    if binary is None:
        pytest.skip("real gitleaks unavailable; release check remains unverified")
    source = Path(__file__).resolve().parents[1]
    config = source / ".gitleaks.toml"
    reviewed = [
        "scripts/canary.sh",
        "scripts/publish.sh",
        "tests/protocol/test_network_v2.py",
        "tests/gpu_ops/test_gpu_ops.py",
        "tests/storage/fakes.py",
        "tests/protocol/fixtures/network_v1_baseline.json",
    ]
    for relative in reviewed:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes((source / relative).read_bytes())

    def scan() -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                binary,
                "detect",
                "--no-git",
                "--source",
                str(tmp_path),
                "--config",
                str(config),
                "--log-level",
                "error",
                "--redact=100",
            ],
            cwd=tmp_path,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
            env={
                k: v
                for k, v in os.environ.items()
                if k not in ("GITLEAKS_CONFIG", "GITLEAKS_CONFIG_TOML")
            },
        )

    # When / Then: exact reviewed files, including all seven original findings, scan clean.
    clean = scan()
    assert clean.returncode == 0, clean.stderr
    fake = FAKE
    contexts = [
        ("scripts/publish.sh", "d3030488eb18aaafc0feceb256666bcc63fc15c4e6ae0bb008980affd96976a2"),
        (
            "tests/protocol/fixtures/network_v1_baseline.json",
            "d3030488eb18aaafc0feceb256666bcc63fc15c4e6ae0bb008980affd96976a2",
        ),
        ("tests/gpu_ops/test_gpu_ops.py", "7875e0437f396a1d"),
        ("tests/storage/fakes.py", "SECRETSECRETSECRETSECRET1234567890abcd"),
    ]
    for relative, value in contexts:
        path = tmp_path / relative
        original = path.read_text()
        line = next(x for x in original.splitlines() if value in x)
        # Added same-line API key is a separate finding; unchanged public scalar stays allowed.
        path.write_text(original.replace(line, line + "; " + API_KEY_NAME + '="' + fake + '"'))
        assert scan().returncode == 1
        # A different value at that very same file/line/rule is not exempt.
        path.write_text(original.replace(line, line.replace(value, fake)))
        assert scan().returncode == 1
        path.write_text(original)
    # A known synthetic value on a different file is not a global token exemption.
    (tmp_path / "other.py").write_text("sec" + 'ret = "' + "SECRET" * 4 + '1234567890abcd"')
    assert scan().returncode == 1


SCANNER_CASES = [
    (
        "deploy/k8s/versions.json",
        "server_build_job",
        (
            "https://github.com/"
            + "CortexLM/hypertrain/actions/runs/"
            + "37815568607/job/113454067723"
        ),
        None,
    ),
    (
        "docs/network-v2.md",
        "--freeze",
        (
            "/root/distributed-decision-training/"
            + ".omo/evidence/hypertrain-network/"
            + "D2-FINAL-SOURCE-FREEZE.json"
        ),
        None,
    ),
    (
        "docs/network-v2.md",
        "--freeze",
        (
            "/root/distributed-decision-training/"
            + ".omo/evidence/hypertrain-network/"
            + "D2-FINAL-SOURCE-FREEZE.json"
        ),
        None,
    ),
    (
        "scripts/network_gpu_prepare.py",
        "operation_receipts",
        "CREATED_ONLY_AT_RUNTIME_FROM_ACCEPTED_CONTEXTS_BY_" + "service_factory",
        None,
    ),
    ("tests/miner/test_island_launch_v2.py", "experiment", "hypertrain-network-v2", "monkeypatch"),
    (
        "tests/protocol/fixtures/network_v1_baseline.json",
        "protocol/keys.py",
        "d3030488eb18aaafc0feceb256666bcc63fc15c4e6ae0bb008980affd96976a2",
        "source_sha256",
    ),
]


def run_scanner(root: Path, path: str, content: str) -> subprocess.CompletedProcess[str]:
    source = Path(__file__).resolve().parents[1]
    script = (source / "scripts/publish.sh").read_text()
    scanner = script[script.index("scan() {") : script.index("\n# raw tree")]
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)
    (root / ".secretscan-allow").write_text("")
    # Actual scanner, not a second token or exception implementation.
    result = subprocess.run(
        [
            "bash",
            "-c",
            'ROOT="$PWD"; SCAN_PATH="$1"; list_files() { printf "%s\\n" "$SCAN_PATH"; }; '
            + scanner
            + "\nscan",
            "scanner",
            path,
        ],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.stderr == "", result.stderr
    return result


def scanner_content(path: str, key: str, value: str, parent: str | None) -> str:
    if path.endswith(".json"):
        payload = {key: value}
        return json.dumps({parent: payload} if parent else payload, indent=2)
    if path.endswith(".md"):
        return "  " + key + " " + value + " \\\n"
    declaration = '    "' + key + '": "' + value + '",\n'
    return (parent + ":\n" if parent else "") + declaration


@pytest.mark.parametrize("path,key,value,parent", SCANNER_CASES)
def test_scanner_exact_public_context_only(
    tmp_path: Path,
    path: str,
    key: str,
    value: str,
    parent: str | None,
) -> None:
    # Given / When: six reviewed scalar contexts pass without whole-file exemption.
    content = scanner_content(path, key, value, parent)
    assert run_scanner(tmp_path, path, content).returncode == 0
    # Then: value/context/file mutation loses the specific exemption.
    assert run_scanner(tmp_path, path, content.replace(value, value + "9")).returncode == 1
    assert run_scanner(tmp_path, path, content.replace(key, "api_key")).returncode == 1
    assert run_scanner(tmp_path, "other/" + path, content).returncode == 1


@pytest.mark.parametrize("path,key,value,parent", SCANNER_CASES)
@pytest.mark.parametrize("secret", ["github", "private", "hex"])
def test_scanner_public_scalar_never_hides_same_line_secret(
    tmp_path: Path,
    path: str,
    key: str,
    value: str,
    parent: str | None,
    secret: str,
) -> None:
    content = scanner_content(path, key, value, parent)
    # Given: generated fake provider token/private key/credential hex after public scalar.
    suffix = {
        "github": " ghp" + "_" + "Ab2Cd3" * 6,
        "private": " " + BEGIN + " " + "PRIVATE KEY-----",
        "hex": " " + API_KEY_NAME + '="' + "a1" * 32 + '"',
    }[secret]
    line = next(line for line in content.splitlines() if value in line)
    content = content.replace(line, line + suffix)
    # When / Then: original raw secret patterns still see appended bytes.
    assert run_scanner(tmp_path, path, content).returncode == 1


def test_scanner_json_parent_and_other_credential_controls(tmp_path: Path) -> None:
    path, key, value, parent = SCANNER_CASES[-1]
    # Wrong JSON parent must not turn a credential-looking filename into public metadata.
    assert run_scanner(tmp_path, path, scanner_content(path, key, value, "private")).returncode == 1
    for content in (
        API_KEY_NAME + ' = "' + "b2" * 32 + '"',
        "monkeypatch:\n    tok" + 'en: "' + "A1b2C3d4" * 5 + '"',
        'secret:\n    digest: "' + "c3" * 32 + '"',
    ):
        assert run_scanner(tmp_path, "tests/control.py", content).returncode == 1


def test_scanner_exact_public_entropy_never_exempts_other_values(tmp_path: Path) -> None:
    import ast
    import hashlib
    import re

    source = Path(__file__).resolve().parents[1]
    script = (source / "scripts/publish.sh").read_text()
    tokens = ast.literal_eval(
        script.split("public_entropy=", 1)[1].split("\npublic_token_grammar=", 1)[0].strip()
    )
    checked = set()
    contexts = []
    for relative, allowed in tokens.items():
        for line in (source / relative).read_text().splitlines():
            for token in re.findall(r"[A-Za-z0-9+/_=-]{40,}", line):
                digest = hashlib.sha256(token.encode()).hexdigest()
                if digest not in allowed:
                    continue
                checked.add((relative, digest))
                contexts.append((relative, line, token))
    qualification = "qualification-usd50-20261009T181257Z/" + "NEXT30-008-RESULT-VERIFY"
    expected = {
        ("docs/RESULTS.md", qualification),
        ("docs/network-v2.md", qualification),
        (
            "docs/network-v2.md",
            "import/source109/TLS/CNI/upload/" + "fixture-master/failover/GC/deny",
        ),
        (
            "scripts/relay_kind_smoke.py",
            "--setenv=KIND_EXPERIMENTAL_PROVIDER=" + "docker",
        ),
        (
            "scripts/verify_image.sh",
            "com/CortexLM/opendecision/archive/" + "e125ff756d57b726fbce44c430e3d2207e7bbda8",
        ),
    }
    assert checked == {
        (relative, hashlib.sha256(token.encode()).hexdigest()) for relative, token in expected
    }
    # Removed by canonical allocation extraction; still exercise its exact
    # public-path exception and every secret control as an explicit fixture.
    legacy = "omo/evidence/hypertrain-network/" + "qualification-usd50-20261009T181257Z"
    relative = "tests/gpu_ops/test_network_admit_cli.py"
    assert hashlib.sha256(legacy.encode()).hexdigest() in tokens[relative]
    contexts.append((relative, 'retained = ".' + legacy + '"', legacy))
    for relative, line, token in contexts:
        assert run_scanner(tmp_path, relative, line + "\n").returncode == 0
        assert run_scanner(tmp_path, relative, "public note: " + line + "\n").returncode == 0
        assert run_scanner(tmp_path, "other/" + relative, line + "\n").returncode == 1
        assert run_scanner(tmp_path, relative, line.replace(token, token + "9")).returncode == 1
        fake = FAKE
        assert (
            run_scanner(
                tmp_path, relative, line + "; " + API_KEY_NAME + '="' + fake + '"'
            ).returncode
            == 1
        )
        assert run_scanner(tmp_path, relative, line + " ghp_" + "Ab2Cd3" * 6).returncode == 1
        assert (
            run_scanner(
                tmp_path, relative, line + " " + BEGIN + " " + "PRIVATE KEY-----"
            ).returncode
            == 1
        )


@pytest.mark.parametrize(
    "content",
    [
        # Digit-free 30-char value under a credential key: used to be skipped.
        API_KEY_NAME + ' = "' + DIGIT_FREE + '"\n',
        API_KEY_NAME + ": " + DIGIT_FREE + "\n",
        "secr" + "et:\n  value: hunter" + "two\n",  # inherited from a credential mapping header
        "pass" + 'word = "short"\n',
        "x = " + '"' + "Zp9Qw2Lm7Xc" + "4Vb8Nk3Rt6Yh1" + '"\n',  # entropy >= 4.0 over 24 chars
        "x = 'gh" + "p_" + "abcdefghijklmnopqrstuvwxyzABCD'\n",
        "x = 'github" + "_pat_" + "abcdefghijklmnopqrstuvwxyz'\n",
        "x = 's" + "k-" + "abcdefghijklmnopqrstuv'\n",
        "x = 'h" + "f_" + "abcdefghijklmnopqrstuv'\n",
        "x = 'AK" + "IA" + "ABCDEFGHIJKLMNOP'\n",
        "x = 'xo" + "xb-" + "abcdefghijkl'\n",
        BEGIN + " CERTIFICATE-----\n",
    ],
)
def test_strict_scan_fails_credentials_entropy_and_prefixes(tmp_path: Path, content: str) -> None:
    assert run_scanner(tmp_path, "conf/settings.yaml", content).returncode == 1


@pytest.mark.parametrize(
    "value",
    [
        '"<redacted>"',
        '"changeme"',
        '"${API_TOKEN}"',
        '"/path/to/token"',
        '"/run/secrets/coord.key"',
        '"miner.token"',
        '""',
        "true",
        '"REPLACE_WITH_TOKEN"',
    ],
)
def test_strict_scan_placeholders_pass(tmp_path: Path, value: str) -> None:
    assert (
        run_scanner(tmp_path, "conf/settings.yaml", API_KEY_NAME + ": " + value + "\n").returncode
        == 0
    )


def test_reviewed_allowlist_is_exact_path_and_line(tmp_path: Path) -> None:
    import hashlib

    # Reviewed entries exempt the strict rules only; legacy provider patterns still apply.
    line = "auth_tok" + 'en = "' + DIGIT_FREE + '"'
    entry = "conf/a.py:" + hashlib.sha256(line.encode()).hexdigest()
    (tmp_path / "scripts").mkdir()
    (tmp_path / "scripts/publish-allowlist.txt").write_text(entry + "  # reviewed\n")
    assert run_scanner(tmp_path, "conf/a.py", line + "\n").returncode == 0
    assert run_scanner(tmp_path, "conf/b.py", line + "\n").returncode == 1
    assert (
        run_scanner(
            tmp_path, "conf/a.py", line.replace(DIGIT_FREE, DIGIT_FREE[::-1]) + "\n"
        ).returncode
        == 1
    )


def test_current_repo_tree_scan_clean() -> None:
    source = Path(__file__).resolve().parents[1]
    script = (source / "scripts/publish.sh").read_text()
    functions = script[script.index("list_files() {") : script.index("\n# raw tree")]
    result = subprocess.run(
        ["bash", "-c", "set -euo pipefail\nROOT=$1\n" + functions + "\nscan", "scan", str(source)],
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_actual_publisher_selector_preserves_public_excludes_private(tmp_path: Path) -> None:
    source = Path(__file__).resolve().parents[1]
    script = (source / "scripts/publish.sh").read_text()
    # Execute the actual function, not a second implementation of its regex rules.
    selector = script[script.index("list_files() {") : script.index("\ncopy_files() {")]
    (tmp_path / ".publishignore").write_bytes((source / ".publishignore").read_bytes())
    (tmp_path / ".secretscan-allow").write_bytes((source / ".secretscan-allow").read_bytes())
    public = {
        ".publishignore",
        ".secretscan-allow",
        "README.md",
        "src/hypertrain/data/store.py",
        "tests/data/test_store.py",
        "tests/protocol/fixtures/network_v1_baseline.json",
        "tests/beacon/vectors.json",
        "tests/gpu_ops/fixtures/leftover-instance.json",
        "tests/fixtures/miner.toml",
        "tests/models/od_fixtures.py",
        "tests/challenge/test_authority_snapshot_v2.py",
        "docs/schemas/example-run-manifest.json",
        "docs/operator.md",
        "deploy/trust-root-v3-row.example.toml",
        "deploy/k8s/local/fixtures.yaml",
        "scripts/network_authority_snapshot.py",
        "experiments/gpu_network_v2/profile.json",
    }
    private = {
        ".omo/evidence/local/proof.json",
        ".herdr-web-ui/session.json",
        "private/config.json",
        "secrets/object.json",
        "roles/role.json",
        "role-keys/coordinator",
        "private-keys/operator",
        "authority/original.json",
        "authority-state/state.json",
        "portable-authority/objects/original",
        "portable-bundle/config.json",
        "portable-bundles/bundle-a/object",
        "snapshots/original.json",
        "preserved-snapshots/original.json",
        "evidence/index.json",
        "rescue/service/object",
        "service-export/objects/original",
        "integration-root/roles/original",
        "experiments/gpu_network_v2/rescue/result.json",
        "experiments/gpu_network_v2/evidence/index.json",
        "experiments/gpu_network_v2/snapshots/original.json",
        "experiments/gpu_network_v2/secrets/object.json",
        "experiments/gpu_network_v2/roles/role.json",
        "experiments/gpu_network_v2/private-roles/coordinator",
        "experiments/gpu_network_v2/portable-bundle/object",
        "local/hot-0.seed",
        "local/coordinator.priv",
        "local/certificate.p12",
        "local/certificate.pfx",
        "local/secret.pkcs8",
        "local/private_key",
        "local/private-key",
        "local/admin.token",
        "local/drain.token",
        "local/challenge.db",
        "local/challenge.db-wal",
        "local/challenge.db-shm",
        "local/bundle.json",
        "local/key.pem",
        "local/operator.key",
        "local/id_ed25519",
        "local/known_hosts",
        "data/raw/public-but-not-publishable",
    }
    for relative in (public | private) - {".publishignore", ".secretscan-allow"}:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("synthetic selector input\n")
    # Neither a file link nor a directory link imports its excluded target.
    (tmp_path / "docs/linked-role.json").symlink_to(tmp_path / "secrets/object.json")
    (tmp_path / "docs/linked-roles").symlink_to(tmp_path / "roles", target_is_directory=True)
    selected = subprocess.run(
        [
            "bash",
            "-c",
            "set -euo pipefail\nROOT=$1\n" + selector + "\nlist_files",
            "selector",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    ).stdout.splitlines()
    assert set(selected) == public
    assert selected == sorted(public)


def test_root_transport_barrier_excluded_from_selected_copy(tmp_path: Path) -> None:
    # Given: synthetic custody only; never inspect the active root barrier.
    repo = Path(__file__).resolve().parents[1]
    script = (repo / "scripts/publish.sh").read_text()
    functions = script[script.index("list_files() {") : script.index("\nscan() {")]
    source, destination = tmp_path / "source", tmp_path / "copy"
    source.mkdir()
    destination.mkdir()
    (source / ".publishignore").write_bytes((repo / ".publishignore").read_bytes())
    (source / ".transport-admission.json").write_bytes(b"PRIVATE\n")
    public = source / "docs/.transport-admission.json"
    public.parent.mkdir()
    public.write_bytes(b"PUBLIC\n")
    # When: original dry selection and copy functions, no scan/remote/publish body.
    result = subprocess.run(
        [
            "bash",
            "-c",
            "set -euo pipefail\nROOT=$1\n"
            + functions
            + '\nlist_files | tee "$2/selected.txt" | copy_files "$2"',
            "copy",
            str(source),
            str(destination),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    # Then: exact root excluded; nested public names and bytes still ship.
    assert (destination / "selected.txt").read_text().splitlines() == [
        ".publishignore",
        "docs/.transport-admission.json",
    ]
    assert not (destination / ".transport-admission.json").exists()
    assert (destination / "docs/.transport-admission.json").read_bytes() == b"PUBLIC\n"
    assert result.stderr == ""


def test_bare_selected_copy_has_publisher_runtime_controls(tmp_path: Path) -> None:
    # Given: actual selector/copier, empty destination; no injected control files.
    source = Path(__file__).resolve().parents[1]
    script = (source / "scripts/publish.sh").read_text()
    functions = script[script.index("list_files() {") : script.index("\n# raw tree")]
    subprocess.run(
        [
            "bash",
            "-c",
            "set -euo pipefail\nROOT=$1\n" + functions + '\nlist_files | copy_files "$2"',
            "copy",
            str(source),
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    # When: shipped functions run entirely from the bare selected tree.
    copied_script = (tmp_path / "scripts/publish.sh").read_text()
    copied_functions = copied_script[
        copied_script.index("list_files() {") : copied_script.index("\n# raw tree")
    ]
    result = subprocess.run(
        ["bash", "-c", "set -euo pipefail\nROOT=$PWD\n" + copied_functions + "\nlist_files\nscan"],
        cwd=tmp_path,
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    # Then: runtime inputs ship byte-exactly; no external selector/scanner policy.
    for control in (".publishignore", ".secretscan-allow"):
        assert control in result.stdout.splitlines()
        assert (tmp_path / control).read_bytes() == (source / control).read_bytes()
    assert result.stderr == ""


@pytest.mark.parametrize("replacement", ["none", "file", "parent"])
def test_actual_copy_rejects_post_selection_symlinks(tmp_path: Path, replacement: str) -> None:
    repo = Path(__file__).resolve().parents[1]
    script = (repo / "scripts/publish.sh").read_text()
    functions = script[script.index("list_files() {") : script.index("\nscan() {")]
    source, destination, outside = (tmp_path / name for name in ("source", "copy", "outside"))
    for directory in (source, destination, outside):
        directory.mkdir()
    (source / ".publishignore").write_bytes((repo / ".publishignore").read_bytes())
    public = source / "docs/operator.md"
    public.parent.mkdir()
    public.write_bytes(b"public operator example\n")
    public.chmod(0o755)
    external = outside / "operator.md"
    external.write_bytes(b"synthetic private external bytes\n")
    command = ["bash", "-c", "set -euo pipefail\nROOT=$1\n" + functions]
    # Selection completion is the deterministic hook; mutate before actual copy.
    selected = subprocess.run(
        [*command[:-1], command[-1] + "\nlist_files", "selector", str(source)],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    ).stdout
    assert selected == ".publishignore\ndocs/operator.md\n"
    if replacement == "file":
        public.unlink()
        public.symlink_to(external)
    elif replacement == "parent":
        public.parent.rename(source / "previous-docs")
        (source / "docs").symlink_to(outside, target_is_directory=True)
    copied = subprocess.run(
        [*command[:-1], command[-1] + '\ncopy_files "$2"', "copy", str(source), str(destination)],
        input=selected,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if replacement == "none":
        assert copied.returncode == 0, copied.stderr
        assert (destination / "docs/operator.md").read_bytes() == b"public operator example\n"
        assert (destination / "docs/operator.md").stat().st_mode & 0o777 == 0o755
    else:
        assert copied.returncode != 0
        assert not (destination / "docs/operator.md").exists()
    assert all(
        path.read_bytes() != external.read_bytes()
        for path in destination.rglob("*")
        if path.is_file()
    )
