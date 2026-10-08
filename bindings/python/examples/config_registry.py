"""Config registry: versioned configs, rollback by carry-forward, tamper evidence."""
import tempfile

import velo

PATH = "configs/service.yaml"
VERSIONS = {
    "1.0.0": "replicas: 2\ntimeout: 30\n",
    "1.1.0": "replicas: 3\ntimeout: 30\n",
    "1.2.0": "replicas: 3\ntimeout: 5\n",  # a bad change
}

with tempfile.TemporaryDirectory() as tmp:
    repo = velo.Repo.init(tmp)

    # Publish: one snapshot per version on the `registry` branch. The version
    # lives in metadata (the binding exposes branches, not tags).
    ids = {}
    parent = None
    for version, body in VERSIONS.items():
        parent = repo.save_tree(
            branch="registry", message=f"publish {version}",
            entries={PATH: body, "configs/limits.yaml": "cpu: 1\n"},
            parent=parent,
            author=velo.Author("release-bot"),
            meta={"registry": {"version": version, "approved_by": "alice"}},
        )
        ids[version] = parent
    assert repo.branch_tip("registry") == ids["1.2.0"]

    # Look a release up by metadata.
    (hit,) = repo.find_snapshots([("registry", "version", "1.1.0")])
    assert hit.id == ids["1.1.0"]

    # Roll back to 1.1.0: carry forward the stored objects, no copying.
    old = repo.tree_at(ids["1.1.0"])
    entries = [velo.TreeEntry.stored(f.path, f.object, f.kind) for f in old]
    rolled = repo.save_tree(
        branch="registry", message="rollback to 1.1.0", entries=entries,
        parent=ids["1.2.0"], author=velo.Author("oncall"),
        meta={"registry": {"version": "1.1.0", "approved_by": "bob",
                           "rollback_of": "1.2.0"}},
    )
    assert repo.read_file_at(rolled, PATH) == VERSIONS["1.1.0"].encode()
    assert repo.read_file_at(rolled, PATH) == repo.read_file_at(ids["1.1.0"], PATH)
    # History is append-only: the bad 1.2.0 is still there to audit.
    assert repo.read_file_at(ids["1.2.0"], PATH) == VERSIONS["1.2.0"].encode()

    # Audit: who approved each state, newest first.
    trail = []
    for e in repo.history(branch="registry"):
        m = repo.snapshot_meta(e.id)["registry"]
        trail.append((m["version"], m["approved_by"]))
        print(e.id[:12], m["version"], m["approved_by"])
    assert trail == [("1.1.0", "bob"), ("1.2.0", "alice"),
                     ("1.1.0", "alice"), ("1.0.0", "alice")]

    # Tamper evidence: the same content with a different approver is a
    # different snapshot id, so history cannot be quietly rewritten.
    forged = repo.save_tree(
        branch="forged", message="publish 1.2.0",
        entries={PATH: VERSIONS["1.2.0"], "configs/limits.yaml": "cpu: 1\n"},
        parent=ids["1.1.0"], author=velo.Author("release-bot"),
        meta={"registry": {"version": "1.2.0", "approved_by": "mallory"}},
    )
    assert forged != ids["1.2.0"]
    assert repo.snapshot_meta(ids["1.2.0"])["registry"]["approved_by"] == "alice"
    print("tamper detected: ids differ", forged[:12], ids["1.2.0"][:12])

    # Release the database before the directory is removed (Windows).
    del repo
