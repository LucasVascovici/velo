import pytest
import velo


def make(tmp_path, name):
    d = tmp_path / name
    d.mkdir()
    return velo.Repo.init(d)


def objects(repo, snap):
    return {f.path: f.object for f in repo.tree_at(snap)}


def test_dict_form_written_matches_tree(tmp_path):
    repo = make(tmp_path, "a")
    base = repo.save_tree(
        branch="main", message="base", entries={"a.txt": "one", "b.txt": "two"}
    )
    saved = repo.save_changes(
        branch="main", message="edit", parent=base,
        changes={"a.txt": "ONE", "c.bin": b"\x00\x01", "b.txt": None},
    )
    assert isinstance(saved, velo.Saved)
    assert "Saved" in repr(saved)
    tree = objects(repo, saved.snapshot)
    assert sorted(tree) == ["a.txt", "c.bin"]
    assert saved.written == {"a.txt": tree["a.txt"], "c.bin": tree["c.bin"]}
    assert repo.branch_tip("main") == saved.snapshot


def test_root_snapshot_without_parent(tmp_path):
    repo = make(tmp_path, "a")
    saved = repo.save_changes(branch="main", message="m", changes={"x": "y"})
    assert repo.snapshot(saved.snapshot).parent is None


def test_change_list_with_rename_and_delete(tmp_path):
    repo = make(tmp_path, "a")
    base = repo.save_tree(
        branch="main", message="base",
        entries={"old.txt": "keep me", "gone.txt": "bye"},
    )
    old_obj = objects(repo, base)["old.txt"]
    changes = [
        velo.Change.rename("old.txt", "new.txt"),
        velo.Change.delete("gone.txt"),
        velo.Change.put("run.sh", "#!/bin/sh\n", kind="executable"),
        velo.Change.stored("alias.txt", old_obj),
    ]
    assert "rename" in repr(changes[0])
    saved = repo.save_changes(
        branch="main", message="m", parent=base, changes=changes
    )
    tree = objects(repo, saved.snapshot)
    assert sorted(tree) == ["alias.txt", "new.txt", "run.sh"]
    assert tree["new.txt"] == old_obj
    for path, obj in saved.written.items():
        assert tree[path] == obj
    assert "run.sh" in saved.written


def test_same_id_as_save_tree(tmp_path):
    one = make(tmp_path, "one")
    two = make(tmp_path, "two")
    kw = dict(branch="main", timestamp_ms=1_700_000_000_000,
              author=velo.Author("ada"))
    base = one.save_tree(message="base", entries={"a": "1", "b": "2"}, **kw)
    base2 = two.save_tree(message="base", entries={"a": "1", "b": "2"}, **kw)
    assert base == base2
    saved = one.save_changes(
        message="next", parent=base, changes={"a": "9", "b": None, "c": "3"}, **kw
    )
    direct = two.save_tree(
        message="next", parent=base2, entries={"a": "9", "c": "3"}, **kw
    )
    assert saved.snapshot == direct


def test_delete_absent_is_invalid_input(tmp_path):
    repo = make(tmp_path, "a")
    base = repo.save_tree(branch="main", message="b", entries={"a": "1"})
    with pytest.raises(velo.InvalidInput):
        repo.save_changes(
            branch="main", message="m", parent=base, changes={"nope": None}
        )
    with pytest.raises(velo.InvalidInput):
        repo.save_changes(
            branch="main", message="m", parent=base,
            changes=[velo.Change.delete("nope")],
        )


def test_bad_changes_type(tmp_path):
    repo = make(tmp_path, "a")
    with pytest.raises(TypeError):
        repo.save_changes(branch="main", message="m", changes=["x"])
