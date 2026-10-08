import pytest
import velo


def make(tmp_path):
    d = tmp_path / "r"
    d.mkdir()
    return velo.Repo.init(d)


def fork(repo, base_files, ours_files, theirs_files):
    a = repo.save_tree(branch="main", message="a", entries=base_files)
    b = repo.save_tree(branch="side", message="b", parent=a, entries=theirs_files)
    c = repo.save_tree(branch="main", message="c", parent=a, entries=ours_files)
    return a, b, c


def test_clean_plan_then_commit(tmp_path):
    repo = make(tmp_path)
    a, b, c = fork(repo, {"x": "1"}, {"x": "1", "o": "o"}, {"x": "1", "t": "t"})
    assert repo.merge_base(c, b) == a
    plan = repo.merge_plan(c, b)
    assert plan.is_clean and plan.conflicts == [] and plan.base == a
    assert [(f.path, f.action) for f in plan.files] == [("t", "added")]
    assert plan.files[0].object is not None
    m = repo.merge_commit(branch="main", ours=c, theirs=b, message="merge")
    assert repo.merge_base(m, b) == b
    assert repo.read_file_at(m, "t") == b"t" and repo.read_file_at(m, "o") == b"o"
    assert repo.snapshot(m).merge_parent == b


def test_conflict_needs_resolution(tmp_path):
    repo = make(tmp_path)
    a, b, c = fork(repo, {"f": "base\n"}, {"f": "ours\n"}, {"f": "theirs\n"})
    plan = repo.merge_plan(c, b)
    assert not plan.is_clean
    (conflict,) = plan.conflicts
    assert conflict.action == "conflicted" and conflict.path == "f"
    assert repo.read_object(conflict.base) == b"base\n"
    assert repo.read_object(conflict.ours) == b"ours\n"
    assert repo.read_object(conflict.theirs) == b"theirs\n"
    with pytest.raises(velo.Conflicts) as exc:
        repo.merge_commit(branch="main", ours=c, theirs=b, message="m")
    assert exc.value.paths == ["f"]
    m = repo.merge_commit(branch="main", ours=c, theirs=b, message="m",
                          resolutions={"f": b"resolved\n"})
    assert repo.read_file_at(m, "f") == b"resolved\n"
    assert repo.merge_base(m, b) == b
    m2 = repo.merge_commit(branch="x", ours=c, theirs=b, message="m",
                           resolutions={"f": "theirs"})
    assert repo.read_file_at(m2, "f") == b"theirs\n"
    with pytest.raises(velo.InvalidInput):
        repo.merge_commit(branch="x", ours=c, theirs=b, message="m",
                          resolutions={"f": "bogus"})


def test_branches(tmp_path):
    repo = make(tmp_path)
    a = repo.save_tree(branch="main", message="a", entries={"f": "1"})
    b = repo.save_tree(branch="main", message="b", parent=a, entries={"f": "2"})
    repo.create_branch("release", at=a)
    repo.create_branch("unborn")
    tips = {br.name: br for br in repo.branches()}
    assert tips["release"].tip == a and tips["main"].tip == b
    assert tips["unborn"].tip is None
    repo.set_branch_tip("release", b)
    assert repo.branch_tip("release") == b
    with pytest.raises(velo.NotFound):
        repo.set_branch_tip("release", "0" * 64)
    with pytest.raises(velo.NotFound):
        repo.set_branch_tip("nope", a)
