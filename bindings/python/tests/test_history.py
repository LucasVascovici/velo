import velo


def make(tmp_path):
    d = tmp_path / "r"
    d.mkdir()
    return velo.Repo.init(d)


def test_history_across_merge_includes_both_sides(tmp_path):
    repo = make(tmp_path)
    a = repo.save_tree(branch="main", message="a", entries={"a": "1"})
    b = repo.save_tree(branch="side", message="b", parent=a, entries={"a": "1", "b": "2"})
    c = repo.save_tree(branch="main", message="c", parent=a, entries={"a": "1", "c": "3"})
    m = repo.merge_commit(branch="main", ours=c, theirs=b, message="m")
    entries = repo.history(from_=m)
    assert {e.id for e in entries} == {a, b, c, m}
    assert entries[0].id == m and entries[0].is_merge
    assert [e.id for e in repo.history(branch="side")] == [b]
    assert len(repo.history(all=True)) == 4
    # The merge itself brings "b" in relative to its first parent.
    assert {e.id for e in repo.history(from_=m, paths=["b"])} == {b, m}


def test_meta_filter_with_limit_returns_newest_matching(tmp_path):
    repo = make(tmp_path)
    parent = None
    ids = []
    for i in range(5):
        kind = "keep" if i % 2 == 0 else "skip"
        parent = repo.save_tree(
            branch="main", message=f"s{i}", parent=parent,
            entries={"f": str(i)}, meta={"app": {"kind": kind}},
        )
        ids.append(parent)
    got = repo.history(from_=parent, limit=2, meta=[("app", "kind", "keep")])
    assert [e.id for e in got] == [ids[4], ids[2]]
    assert len(repo.history(from_=parent, meta=[("app", "kind")])) == 5


def test_find_snapshots(tmp_path):
    repo = make(tmp_path)
    a = repo.save_tree(branch="main", message="a", entries={"f": "1"},
                       meta={"app": {"k": "x", "j": "1"}})
    repo.save_tree(branch="main", message="b", parent=a, entries={"f": "2"},
                   meta={"app": {"k": "y", "j": "1"}})
    assert [e.id for e in repo.find_snapshots([("app", "k", "x")])] == [a]
    assert len(repo.find_snapshots([("app", "j", "1")])) == 2
    assert repo.find_snapshots([("app", "k", "x"), ("app", "j", "2")]) == []


def test_blame_attribution_author_and_window(tmp_path):
    repo = make(tmp_path)
    ada, bob = velo.Author("ada", "ada@x.org"), velo.Author("bob")
    a = repo.save_tree(branch="main", message="first", entries={"n": "one\ntwo\n"},
                       author=ada)
    b = repo.save_tree(branch="feat", message="second", parent=a,
                       entries={"n": "one\ntwo\nthree\n"}, author=bob)
    blame = repo.blame("n", at=b)
    assert blame.snapshot == b and blame.path == "n"
    assert [l.text for l in blame.lines] == ["one", "two", "three"]
    first, last = blame.lines[0].origin, blame.lines[2].origin
    assert first.id == a and first.author.name == "ada"
    assert first.author.email == "ada@x.org" and first.branch == "main"
    assert last.id == b and last.author.name == "bob" and last.message == "second"
    assert last.branch == "feat" and last.created_at.tzinfo is not None
    window = repo.blame("n", at=b, lines=(2, 3))
    assert [l.line_no for l in window.lines] == [2, 3]
    assert blame.lines[0].line_count == 1
