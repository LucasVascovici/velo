import threading

import pytest
import velo


def make(tmp_path, name="r"):
    d = tmp_path / name
    d.mkdir()
    return velo.Repo.init(d)


def test_init_then_open(tmp_path):
    make(tmp_path)
    assert isinstance(velo.Repo.open(tmp_path / "r"), velo.Repo)


def test_open_empty_dir_is_not_a_repo(tmp_path):
    with pytest.raises(velo.NotARepo) as exc:
        velo.Repo.open(tmp_path)
    assert isinstance(exc.value, velo.VeloError)


def test_init_twice(tmp_path):
    make(tmp_path)
    with pytest.raises(velo.AlreadyInitialized):
        velo.Repo.init(tmp_path / "r")


def test_save_tree_roundtrip(tmp_path):
    repo = make(tmp_path)
    sid = repo.save_tree(
        branch="main",
        message="m",
        entries={"a.txt": b"hello", "d/b.txt": "text é"},
    )
    files = {f.path: f for f in repo.tree_at(sid)}
    assert set(files) == {"a.txt", "d/b.txt"}
    assert files["a.txt"].kind == "regular"
    assert repo.read_file_at(sid, "a.txt") == b"hello"
    assert repo.read_file_at(sid, "d/b.txt") == "text é".encode()
    assert repo.read_object(files["a.txt"].object) == b"hello"
    entry = repo.snapshot(sid)
    assert entry.id == sid and entry.message == "m" and not entry.is_merge
    assert entry.created_at.tzinfo is not None
    assert repo.resolve(sid[:12]) == sid
    assert repo.branch_tip("main") == sid


def test_list_entries_and_kinds(tmp_path):
    repo = make(tmp_path)
    sid = repo.save_tree(
        branch="main",
        message="m",
        entries=[
            velo.TreeEntry.file("a", b"1"),
            velo.TreeEntry.executable("run.sh", b"#!/bin/sh\n"),
            velo.TreeEntry.symlink("l", "a"),
        ],
    )
    kinds = {f.path: f.kind for f in repo.tree_at(sid)}
    assert kinds == {"a": "regular", "run.sh": "executable", "l": "symlink"}


def test_stored_carries_file_forward(tmp_path):
    repo = make(tmp_path)
    first = repo.save_tree(branch="main", message="1", entries={"a.txt": b"keep"})
    obj = repo.tree_at(first)[0].object
    second = repo.save_tree(
        branch="main",
        message="2",
        parent=first,
        entries=[velo.TreeEntry.stored("a.txt", obj), velo.TreeEntry.file("b", b"x")],
    )
    assert repo.read_file_at(second, "a.txt") == b"keep"
    assert repo.snapshot(second).parent == first


def test_meta_and_author(tmp_path):
    repo = make(tmp_path)
    sid = repo.save_tree(
        branch="main",
        message="m",
        entries={"a": b"1"},
        meta={"app": {"k": "v"}},
        author=velo.Author("Ada", "ada@example.com"),
    )
    meta = repo.snapshot_meta(sid)
    assert meta["app"] == {"k": "v"}
    assert "Ada" in meta["velo"].values()
    assert "ada@example.com" in meta["velo"].values()


def test_fixed_timestamp_gives_same_id(tmp_path):
    ids = []
    for name in ("one", "two"):
        repo = make(tmp_path, name)
        ids.append(
            repo.save_tree(
                branch="main",
                message="m",
                entries={"a": b"1"},
                timestamp_ms=1_700_000_000_000,
            )
        )
    assert ids[0] == ids[1]
    assert repo.snapshot(ids[0]).created_at_ms == 1_700_000_000_000


def test_repo_is_unsendable(tmp_path):
    repo = make(tmp_path)
    errors = []

    def use():
        try:
            repo.branch_tip("main")
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    t = threading.Thread(target=use)
    t.start()
    t.join()
    assert len(errors) == 1 and isinstance(errors[0], RuntimeError)


def test_unknown_parent_is_not_found(tmp_path):
    repo = make(tmp_path)
    with pytest.raises(velo.NotFound) as exc:
        repo.save_tree(
            branch="main", message="m", entries={"a": b"1"}, parent="a" * 64
        )
    assert exc.value.kind == "snapshot"
    assert isinstance(exc.value.name, str)


def test_bad_id_is_invalid_input(tmp_path):
    repo = make(tmp_path)
    with pytest.raises(velo.InvalidInput):
        repo.tree_at("not an id")


def test_branch_tip_unborn_is_none(tmp_path):
    assert make(tmp_path).branch_tip("main") is None


def test_head_token_is_int(tmp_path):
    assert isinstance(make(tmp_path).head_token(), int)


def test_compacted_exception_exists():
    assert issubclass(velo.Compacted, velo.VeloError)
    assert issubclass(velo.VeloIOError, velo.VeloError)
