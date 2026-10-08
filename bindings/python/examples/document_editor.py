"""Document editor: drafts as branches, a rename edge, merge, blame per line."""
import tempfile

import velo

TEXT = "Intro sentence.\nMiddle sentence.\nClosing sentence.\n"

with tempfile.TemporaryDirectory() as tmp:
    repo = velo.Repo.init(tmp)

    # One sentence per line keeps merges and blame at sentence granularity.
    base = repo.save_tree(
        branch="main", message="first draft", entries={"draft.txt": TEXT},
        author=velo.Author("ada"),
    )

    # Ada renames the file; the rename edge lets blame follow lines back
    # through it.
    renamed = repo.save_tree(
        branch="main", message="rename draft to essay", parent=base,
        entries={"essay.txt": TEXT},
        author=velo.Author("ada"), renames=[("draft.txt", "essay.txt")],
    )

    # Grace works on a draft branch and rewrites the middle sentence.
    repo.create_branch("draft-grace", at=renamed)
    g = repo.save_tree(
        branch="draft-grace", message="tighten the middle", parent=renamed,
        entries={"essay.txt": "Intro sentence.\nA sharper middle.\nClosing sentence.\n"},
        author=velo.Author("grace"),
    )
    # Meanwhile Ada edits the same sentence on main.
    m1 = repo.save_tree(
        branch="main", message="reword the middle", parent=renamed,
        entries={"essay.txt": "Intro sentence.\nAn ada middle.\nClosing sentence.\n"},
        author=velo.Author("ada"),
    )

    plan = repo.merge_plan(m1, g)
    assert not plan.is_clean
    assert [c.path for c in plan.conflicts] == ["essay.txt"]
    try:
        repo.merge_commit(branch="main", ours=m1, theirs=g, message="merge")
        raise AssertionError("expected a conflict")
    except velo.Conflicts as e:
        assert e.paths == ["essay.txt"]

    resolved = b"Intro sentence.\nA sharper middle.\nClosing sentence.\n"
    merged = repo.merge_commit(
        branch="main", ours=m1, theirs=g, message="merge grace's draft",
        author=velo.Author("ada"), resolutions={"essay.txt": resolved},
    )
    entry = repo.snapshot(merged)
    assert entry.is_merge and entry.merge_parent == g
    assert repo.read_file_at(merged, "essay.txt") == resolved

    blame = repo.blame("essay.txt", at=merged)
    who = {}
    for line in blame.lines:
        who[line.text.rstrip()] = (line.origin.author.name, line.origin.branch)
        print(f"{line.line_no}: {line.origin.author.name:<6} "
              f"{line.origin.branch:<12} {line.text.rstrip()}")
    assert who["Intro sentence."][0] == "ada"
    assert who["Closing sentence."][0] == "ada"

    # Across the rename: the first line of essay.txt traces to draft.txt.
    old = repo.blame("essay.txt", at=renamed, lines=(1, 1))
    assert old.lines[0].origin.id == base
    assert old.lines[0].origin.path == "draft.txt"

    # Release the database before the directory is removed (Windows).
    del repo
