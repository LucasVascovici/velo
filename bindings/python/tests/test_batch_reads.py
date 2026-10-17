import pytest
import velo


def make(tmp_path):
    d = tmp_path / "repo"
    d.mkdir()
    repo = velo.Repo.init(d)
    snap = repo.save_tree(
        branch="main",
        message="base",
        entries={
            "a.txt": "one",
            "src/b.py": "two",
            "src/c.bin": b"\x00\xff\x01",
            "docs/d.md": "four",
        },
    )
    return repo, snap


def test_read_files_at_order_and_bytes(tmp_path):
    repo, snap = make(tmp_path)
    paths = ["src/c.bin", "a.txt", "src/b.py", "a.txt"]
    got = repo.read_files_at(snap, paths)
    assert [p for p, _ in got] == paths
    for p, data in got:
        assert isinstance(data, bytes)
        assert data == repo.read_file_at(snap, p)


def test_read_files_at_missing_path_errors(tmp_path):
    repo, snap = make(tmp_path)
    with pytest.raises(velo.VeloError):
        repo.read_files_at(snap, ["a.txt", "nope.txt"])


def test_materialize_writes_whole_tree(tmp_path):
    repo, snap = make(tmp_path)
    dest = tmp_path / "out"
    assert repo.materialize(snap, dest) == 4
    assert (dest / "a.txt").read_text() == "one"
    assert (dest / "src" / "c.bin").read_bytes() == b"\x00\xff\x01"


def test_materialize_only_and_ignore(tmp_path):
    repo, snap = make(tmp_path)
    only = tmp_path / "only"
    assert repo.materialize(snap, only, only=["src/**"]) == 2
    assert not (only / "a.txt").exists()
    assert (only / "src" / "b.py").exists()
    ign = tmp_path / "ign"
    assert repo.materialize(snap, ign, ignore=["*.bin", "docs/"]) == 2
    assert not (ign / "src" / "c.bin").exists()
    assert (ign / "a.txt").exists()
