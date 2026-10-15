import faulthandler
import sys

import pytest
import velo


@pytest.fixture(autouse=True)
def deadlock_guard():
    # A deadlock must fail the run instead of hanging it: after 30 seconds the
    # process dumps every thread's stack and exits.
    faulthandler.dump_traceback_later(30, exit=True)
    yield
    faulthandler.cancel_dump_traceback_later()


def make(tmp_path):
    d = tmp_path / "r"
    d.mkdir()
    return velo.Repo.init(d)


def fork(repo, path, base, ours, theirs):
    a = repo.save_tree(branch="main", message="a", entries={path: base})
    b = repo.save_tree(branch="side", message="b", parent=a, entries={path: theirs})
    c = repo.save_tree(branch="main", message="c", parent=a, entries={path: ours})
    return a, b, c


JSON_BASE = '{\n  "a": 1,\n  "b": 2\n}\n'
JSON_OURS = '{\n  "a": 10,\n  "b": 2\n}\n'
JSON_THEIRS = '{\n  "a": 1,\n  "b": 20\n}\n'


def test_on_event_sees_saved_after_the_call_returns(tmp_path):
    repo = make(tmp_path)
    seen = []
    # The callback may use the repository: it runs outside the core call.
    repo.on_event(lambda e: seen.append((e, repo.branch_tip("main"))))
    assert seen == []
    snap = repo.save_tree(branch="main", message="m", entries={"f": "1"})
    assert len(seen) == 1
    event, tip = seen[0]
    assert event["kind"] == "Saved"
    assert event["snapshot"] == snap and event["branch"] == "main"
    assert event["parent"] is None
    assert tip == snap


def test_on_event_reports_merge(tmp_path):
    repo = make(tmp_path)
    a, b, c = fork(repo, "f", "1\n", "1\n", "1\n")
    seen = []
    repo.on_event(seen.append)
    m = repo.merge_commit(branch="main", ours=c, theirs=b, message="merge")
    assert [e["kind"] for e in seen] == ["Saved", "Merged"]
    assert seen[1]["snapshot"] == m and seen[1]["theirs"] == b


def test_event_callback_error_does_not_fail_the_write(tmp_path):
    repo = make(tmp_path)

    def boom(_):
        raise ValueError("listener bug")

    repo.on_event(boom)
    reported = []
    old = sys.unraisablehook
    sys.unraisablehook = reported.append
    try:
        snap = repo.save_tree(branch="main", message="m", entries={"f": "1"})
    finally:
        sys.unraisablehook = old
    assert len(reported) == 1 and isinstance(reported[0].exc_value, ValueError)
    assert repo.branch_tip("main") == snap


def test_builtin_json_driver_merges_what_lines_cannot(tmp_path):
    repo = make(tmp_path)
    _, b, c = fork(repo, "c.json", JSON_BASE, JSON_OURS, JSON_THEIRS)
    assert not repo.merge_plan(c, b).is_clean
    repo.set_merge_driver("*.json", "json")
    plan = repo.merge_plan(c, b)
    assert plan.is_clean
    (f,) = plan.files
    assert f.action == "auto_merged"
    assert b'"a": 10' in f.content and b'"b": 20' in f.content
    repo.clear_merge_drivers()
    assert not repo.merge_plan(c, b).is_clean


def test_unknown_driver_and_pattern_are_refused(tmp_path):
    repo = make(tmp_path)
    with pytest.raises(velo.InvalidInput):
        repo.set_merge_driver("*.json", "xml")
    with pytest.raises(velo.InvalidInput):
        repo.set_merge_driver("[", "json")
    with pytest.raises(TypeError):
        repo.set_merge_driver("*.json", 3)


def test_custom_driver_merges_and_conflicts(tmp_path):
    repo = make(tmp_path)
    _, b, c = fork(repo, "f.txt", "base\n", "ours\n", "theirs\n")
    calls = []

    def join(ancestor, ours, theirs):
        calls.append((ancestor, ours, theirs))
        return ours.strip() + "+" + theirs.strip() + "\n"

    repo.set_merge_driver("*.txt", join)
    plan = repo.merge_plan(c, b)
    assert plan.is_clean and plan.files[0].content == b"ours+theirs\n"
    assert calls == [("base\n", "ours\n", "theirs\n")]
    m = repo.merge_commit(branch="main", ours=c, theirs=b, message="merge")
    assert repo.read_file_at(m, "f.txt") == b"ours+theirs\n"

    repo.set_merge_driver("*.txt", lambda a, o, t: None)
    # The first matching rule wins, so the earlier function still applies.
    assert repo.merge_plan(c, b).is_clean
    repo.clear_merge_drivers()
    repo.set_merge_driver("*.txt", lambda a, o, t: None)
    plan = repo.merge_plan(c, b)
    assert not plan.is_clean
    with pytest.raises(velo.Conflicts):
        repo.merge_commit(branch="main", ours=c, theirs=b, message="merge")


def test_driver_exception_surfaces_and_nothing_is_committed(tmp_path):
    repo = make(tmp_path)
    _, b, c = fork(repo, "f.txt", "base\n", "ours\n", "theirs\n")
    before = repo.history(all=True)

    def bad(a, o, t):
        raise ValueError("driver bug")

    repo.set_merge_driver("*.txt", bad)
    with pytest.raises(ValueError, match="driver bug"):
        repo.merge_plan(c, b)
    with pytest.raises(ValueError, match="driver bug"):
        repo.merge_commit(branch="main", ours=c, theirs=b, message="merge")
    # Even with a resolution that would settle the conflict.
    with pytest.raises(ValueError, match="driver bug"):
        repo.merge_commit(
            branch="main", ours=c, theirs=b, message="merge",
            resolutions={"f.txt": "ours"},
        )
    assert [e.id for e in repo.history(all=True)] == [e.id for e in before]
    assert repo.branch_tip("main") == c

    repo.set_merge_driver("*.md", lambda a, o, t: 5)
    repo.clear_merge_drivers()
    repo.set_merge_driver("*.txt", lambda a, o, t: 5)
    with pytest.raises(TypeError):
        repo.merge_plan(c, b)


def test_driver_calling_back_into_the_repo_is_refused_not_deadlocked(tmp_path):
    repo = make(tmp_path)
    _, b, c = fork(repo, "f.txt", "base\n", "ours\n", "theirs\n")
    caught = []

    def reenter(a, o, t):
        try:
            repo.branches()
        except velo.ReentrantCall as e:
            caught.append(e)
            raise
        return "unreachable"

    repo.set_merge_driver("*.txt", reenter)
    with pytest.raises(velo.ReentrantCall, match="must not call back"):
        repo.merge_plan(c, b)
    assert len(caught) == 1 and isinstance(caught[0], velo.VeloError)
    # The handle is intact afterwards.
    repo.clear_merge_drivers()
    assert repo.branch_tip("main") == c
    assert not repo.merge_plan(c, b).is_clean
