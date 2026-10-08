# Cookbook: config registry

Script: [`bindings/python/examples/config_registry.py`](../../bindings/python/examples/config_registry.py)

A registry publishes versions of `configs/service.yaml`, rolls back cheaply,
and can answer "who approved this?" in a way nobody can rewrite.

## Publish versions

Each version is a snapshot on the `registry` branch. The version and approver
are metadata:

```python
repo.save_tree(branch="registry", message=f"publish {version}",
               entries={PATH: body, "configs/limits.yaml": "cpu: 1\n"},
               parent=parent, author=velo.Author("release-bot"),
               meta={"registry": {"version": version, "approved_by": "alice"}})
```

The Python binding exposes branches but not tags, so the release version lives
in metadata rather than a tag. A release is then found with
`repo.find_snapshots([("registry", "version", "1.1.0")])`.

## Roll back

Version 1.2.0 turns out to be bad. Roll back by saving a new snapshot whose
entries carry forward the stored objects of 1.1.0, with
`TreeEntry.stored(path, object, kind)`:

```python
entries = [velo.TreeEntry.stored(f.path, f.object, f.kind)
           for f in repo.tree_at(ids["1.1.0"])]
repo.save_tree(branch="registry", message="rollback to 1.1.0", entries=entries,
               parent=ids["1.2.0"], ...)
```

No file content is read or copied; the new snapshot points at the same objects.
History stays append-only: the bad 1.2.0 is still there, and the script asserts
both that the rolled-back content equals 1.1.0 and that 1.2.0 is intact.

## Audit

`repo.history(branch="registry")` plus `snapshot_meta` gives the full trail:
1.0.0 and 1.1.0 and 1.2.0 approved by alice, then the rollback approved by bob.

## Tamper evidence

Metadata is part of the snapshot id. Publishing the same content as 1.2.0 with
`approved_by` set to someone else produces a different id, so the original
approval cannot be swapped out: anyone holding the id of 1.2.0 can verify that
`alice` approved exactly that content. The script asserts the ids differ and
the original metadata is unchanged.
