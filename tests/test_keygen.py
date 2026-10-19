import stat

import pytest

from cortex.config import read_seed
from cortex.keygen import generate_key
from cortex.protocol.crypto import public_key


def test_generates_private_seed_matching_public_key(tmp_path):
    seed, public = tmp_path / "leaf.seed", tmp_path / "leaf.pubkey"
    encoded = generate_key(seed, public)
    assert stat.S_IMODE(seed.stat().st_mode) == 0o600
    assert public.read_text().strip() == encoded == public_key(read_seed(seed)).hex()


def test_never_overwrites_and_leaves_no_orphan_seed(tmp_path):
    seed, public = tmp_path / "leaf.seed", tmp_path / "leaf.pubkey"
    generate_key(seed, public)
    original = seed.read_bytes()
    with pytest.raises(FileExistsError):
        generate_key(seed, tmp_path / "other.pubkey")
    assert seed.read_bytes() == original
    public.write_text("x")
    other = tmp_path / "second.seed"
    with pytest.raises(FileExistsError):
        generate_key(other, public)
    assert not other.exists()
