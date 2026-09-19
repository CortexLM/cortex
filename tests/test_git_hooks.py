"""Local Git hooks isolate fixture repositories from the checkout being pushed."""

import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_pre_push_fixture_git_preserves_linked_repository_and_staged_changes(tmp_path):
    main = tmp_path / "main"
    linked = tmp_path / "linked"
    remote = tmp_path / "remote.git"
    fixture = tmp_path / "fixture"
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        PATH=f"{tmp_path / 'bin'}{os.pathsep}{env['PATH']}",
        CORTEX_HOOK_TEST_FIXTURE=str(fixture),
    )

    def git(*args):
        return subprocess.run(
            ["git", *map(str, args)], env=env, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q", main)
    git("-C", main, "config", "user.name", "Hook test")
    git("-C", main, "config", "user.email", "hook@example.test")
    git("-C", main, "commit", "-q", "--allow-empty", "-m", "initial")
    git("-C", main, "worktree", "add", "-q", "-b", "topic", linked)
    git("init", "-q", "--bare", remote)
    (linked / "pending.txt").write_text("preserve this staged change")
    git("-C", linked, "add", "pending.txt")
    index_tree = git("-C", linked, "write-tree")
    config = (main / ".git/config").read_bytes()
    shutil.copy2(ROOT / ".githooks/pre-push", main / ".git/hooks/pre-push")
    # Stand in for the test runner; Git and the repository's push hook remain real.
    runner = tmp_path / "bin/uv"
    runner.parent.mkdir()
    runner.write_text(
        '#!/bin/sh\nset -eu\n[ "$3" = pytest ] || exit 0\n'
        'git init -q "$CORTEX_HOOK_TEST_FIXTURE"\n'
        'printf fixture > "$CORTEX_HOOK_TEST_FIXTURE/fixture.txt"\n'
        'git -C "$CORTEX_HOOK_TEST_FIXTURE" add fixture.txt\n',
    )
    runner.chmod(0o755)

    git("-C", linked, "push", str(remote), "topic")

    assert (fixture / ".git").is_dir()
    assert (main / ".git/config").read_bytes() == config
    assert git("-C", main, "rev-parse", "--show-toplevel") == str(main.resolve())
    assert git("-C", main, "status", "--porcelain") == ""
    assert git("-C", linked, "write-tree") == index_tree
    assert git("-C", fixture, "diff", "--cached", "--name-only") == "fixture.txt"
    assert git("--git-dir", remote, "rev-parse", "topic") == git("-C", linked, "rev-parse", "HEAD")
