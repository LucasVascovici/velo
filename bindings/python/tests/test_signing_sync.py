"""Snapshot signing and one-step sync (18.8-d).

Signing needs only the binding. Sync needs a working tree, a remote and a
clone, none of which the binding creates, so those scenarios set up their
repositories with the `velody` CLI and skip when no binary is built.
"""

import os
import pathlib
import shutil
import subprocess

import pytest

import velody

SEED = bytes(range(32))
OTHER = bytes(range(1, 33))


def make(tmp_path):
    d = tmp_path / "r"
    d.mkdir()
    return velody.Repo.init(d)


def snapshot(repo):
    return repo.save_tree(branch="main", message="m", entries={"f": "x"})


def test_signing_key_from_seed_and_hex():
    key = velody.SigningKey.from_seed(SEED)
    assert len(key.public_key) == 64
    assert velody.SigningKey.from_hex(SEED.hex()).public_key == key.public_key
    assert key.public_key in repr(key)
    assert SEED.hex() not in repr(key)


def test_bad_seed_is_refused():
    with pytest.raises(velody.InvalidInput):
        velody.SigningKey.from_seed(b"short")
    with pytest.raises(velody.InvalidInput):
        velody.SigningKey.from_hex("nothex")


def test_sign_list_verify_trusted_and_untrusted(tmp_path):
    repo = make(tmp_path)
    snap = snapshot(repo)
    key = velody.SigningKey.from_seed(SEED)
    other = velody.SigningKey.from_seed(OTHER)
    assert repo.signatures(snap) == []
    assert repo.signed_snapshots() == []

    sig = repo.sign(snap, key)
    assert sig.snapshot == snap
    assert sig.public_key == key.public_key
    assert sig.algorithm == "ed25519"
    assert [s.public_key for s in repo.signatures(snap)] == [key.public_key]
    assert repo.signed_snapshots() == [snap]

    trusted = repo.verify(snap, [key.public_key])
    assert trusted.is_trusted
    assert trusted.trusted == [key.public_key]
    assert trusted.untrusted == [] and trusted.invalid == []

    untrusted = repo.verify(snap, [other.public_key])
    assert not untrusted.is_trusted
    assert untrusted.untrusted == [key.public_key]
    assert untrusted.trusted == []

    assert not repo.verify(snap, []).is_trusted


def test_verify_rejects_a_malformed_key(tmp_path):
    repo = make(tmp_path)
    with pytest.raises(velody.InvalidInput):
        repo.verify(snapshot(repo), ["zz"])


# --- sync ---------------------------------------------------------------

def find_velody():
    env = os.environ.get("VELODY_BIN")
    if env:
        return env
    root = pathlib.Path(__file__).resolve().parents[3]
    for profile in ("debug", "release"):
        for name in ("velody.exe", "velody"):
            p = root / "target" / profile / name
            if p.exists():
                return str(p)
    return shutil.which("velody")


VELO = find_velody()
needs_cli = pytest.mark.skipif(VELO is None, reason="no velody binary to set up repositories")


def cli(cwd, *args):
    subprocess.run([VELO, *args], cwd=cwd, check=True, capture_output=True)


def put(root, name, text):
    (root / name).write_text(text)


def tip(root):
    return velody.Repo.open(root).branch_tip("main")


@pytest.fixture
def pair(tmp_path):
    """An origin with one snapshot and a clone of it."""
    origin = tmp_path / "origin"
    origin.mkdir()
    cli(origin, "init")
    put(origin, "a.txt", "shared\n")
    cli(origin, "save", "base")
    cli(tmp_path, "clone", str(origin), "copy")
    return origin, tmp_path / "copy"


@needs_cli
def test_sync_nothing(pair):
    _, copy = pair
    out = velody.Repo.open(copy).sync()
    assert out.kind == "nothing"
    assert out.branch == "main" and out.remote == "origin"


@needs_cli
def test_sync_behind_fast_forwards(pair):
    origin, copy = pair
    repo = velody.Repo.open(copy)
    put(origin, "b.txt", "new\n")
    cli(origin, "save", "theirs")
    out = repo.sync()
    assert out.kind == "fast_forwarded"
    assert out.to == tip(origin)
    assert (copy / "b.txt").read_text() == "new\n"


@needs_cli
def test_sync_ahead_pushes(pair):
    origin, copy = pair
    put(copy, "b.txt", "mine\n")
    cli(copy, "save", "mine")
    out = velody.Repo.open(copy).sync("origin", "main")
    assert out.kind == "pushed"
    assert out.report.outcome == "sent"
    assert out.report.snapshots == 1
    assert tip(origin) == tip(copy)


@needs_cli
def test_sync_diverged_clean_merges(pair):
    origin, copy = pair
    put(origin, "theirs.txt", "t\n")
    cli(origin, "save", "theirs")
    put(copy, "mine.txt", "m\n")
    cli(copy, "save", "mine")
    out = velody.Repo.open(copy).sync(
        author=velody.Author("Ann", "ann@example.com"), message="joined"
    )
    assert out.kind == "merged"
    assert out.merge == tip(origin) == tip(copy)
    assert out.report.outcome == "sent"
    assert (copy / "theirs.txt").exists() and (copy / "mine.txt").exists()


@needs_cli
def test_sync_diverged_conflict_stops(pair):
    origin, copy = pair
    put(origin, "a.txt", "theirs\n")
    cli(origin, "save", "theirs")
    put(copy, "a.txt", "mine\n")
    cli(copy, "save", "mine")
    before = tip(origin)
    out = velody.Repo.open(copy).sync()
    assert out.kind == "conflicted"
    assert out.paths == ["a.txt"]
    assert tip(origin) == before


@needs_cli
def test_sync_token_is_accepted_and_never_in_errors(pair):
    origin, copy = pair
    secret = "s3cr3t-bearer-token"
    out = velody.Repo.open(copy).sync(token=secret)
    assert out.kind == "nothing"
    put(copy, "dirty.txt", "unsaved\n")
    with pytest.raises(velody.DirtyWorkingTree) as err:
        velody.Repo.open(copy).sync(token=secret)
    assert secret not in str(err.value)
    with pytest.raises(velody.VelodyError) as err:
        velody.Repo.open(copy).sync("nowhere", token=secret)
    assert secret not in str(err.value)
