import pytest

import velo

DAY = 86_400_000
NOW = 100 * DAY


def make(tmp_path):
    d = tmp_path / "r"
    d.mkdir()
    return velo.Repo.init(d)


def spread(repo, count=40):
    """One snapshot a day for `count` days, 40 days old down to 1 day old."""
    parent = None
    ids = []
    for i in range(count):
        parent = repo.save_tree(
            branch="main",
            message=f"s{i}",
            entries={"f": str(i)},
            parent=parent,
            timestamp_ms=NOW - (count - i) * DAY,
        )
        ids.append(parent)
    return ids


# Everything older than a day keeps one snapshot per 10 days.
TIERS = [(DAY, 10 * DAY)]


def test_retention_dry_run_reports_and_changes_nothing(tmp_path):
    repo = make(tmp_path)
    ids = spread(repo)
    report = repo.retention("main", TIERS, now_ms=NOW, dry_run=True)
    assert report["dry_run"] is True
    assert report["removed"] > 0 and report["squashed"]
    assert report["examined"] == 40
    assert repo.branch_tip("main") == ids[-1]
    assert len(repo.history(branch="main")) == 40
    assert repo.snapshot(ids[0]).id == ids[0]


def test_retention_run_squashes_and_old_ids_are_compacted(tmp_path):
    repo = make(tmp_path)
    ids = spread(repo)
    report = repo.retention("main", TIERS, now_ms=NOW)
    assert report["removed"] > 0
    assert len(repo.history(branch="main")) == 40 - report["removed"]
    gone = report["squashed"][0]["removed"][0]
    into = report["squashed"][0]["into"]
    with pytest.raises(velo.Compacted) as caught:
        repo.snapshot(gone)
    assert caught.value.id == gone
    assert caught.value.into == into
    assert repo.snapshot(into).id == into
    assert ids[0] in {gone} | {r for s in report["squashed"] for r in s["removed"]}


def test_default_policy_and_protected_tag(tmp_path):
    repo = make(tmp_path)
    ids = spread(repo)
    repo.create_tag("keep", ids[5])
    report = repo.retention("main", now_ms=NOW, dry_run=True)
    assert {"id": ids[5], "reason": "tagged"} in report["protected"]


def test_compact_range(tmp_path):
    repo = make(tmp_path)
    ids = spread(repo, 6)
    dry = repo.compact("main", [(ids[1], ids[3])], dry_run=True)
    assert dry["dry_run"] is True and len(dry["squashed"]) == 1
    assert repo.snapshot(ids[2]).id == ids[2]
    done = repo.compact("main", [(ids[1], ids[3])])
    assert done["squashed"][0]["removed"] == ids[1:4]
    assert done["rewritten"]  # ids[4] and ids[5] were re-minted
    old, new = done["rewritten"][0]
    with pytest.raises(velo.Compacted) as caught:
        repo.snapshot(old)
    assert caught.value.into == new
    assert repo.branch_tip("main") != ids[-1]


def test_bad_tiers_are_invalid_input(tmp_path):
    repo = make(tmp_path)
    spread(repo, 3)
    with pytest.raises(velo.InvalidInput):
        repo.retention("main", [(0, DAY)], now_ms=NOW)


def test_tags(tmp_path):
    repo = make(tmp_path)
    ids = spread(repo, 3)
    assert repo.tags() == []
    assert repo.create_tag("v1", ids[0]) == ids[0]
    repo.create_tag("v2", ids[1][:12])
    assert [(t.name, t.snapshot) for t in repo.tags()] == [("v1", ids[0]), ("v2", ids[1])]
    assert repo.tags()[0].message == "s0"
    with pytest.raises(velo.InvalidInput):
        repo.create_tag("v1", ids[2])
    repo.create_tag("v1", ids[2], force=True)
    assert repo.tags()[0].snapshot == ids[2]
    repo.delete_tag("v1")
    assert [t.name for t in repo.tags()] == ["v2"]
    with pytest.raises(velo.NotFound):
        repo.delete_tag("v1")


def test_fsck_on_a_healthy_repository(tmp_path):
    repo = make(tmp_path)
    spread(repo, 3)
    report = repo.fsck()
    assert report["problems"] == []
    assert repo.fsck(repair=True)["problems"] == []
    assert report["cruft"] == [] and report["repaired"] == []
