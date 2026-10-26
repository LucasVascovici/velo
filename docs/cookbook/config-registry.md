# Cookbook: config registry

Script: [`bindings/python/examples/config_registry.py`](../../bindings/python/examples/config_registry.py)
Node port: [`bindings/node/examples/config_registry.mjs`](../../bindings/node/examples/config_registry.mjs)
(same steps and asserts; its header says how to run it against a local build)

A registry publishes versions of `configs/service.yaml`, rolls back cheaply,
and can answer "who approved this?" in a way nobody can rewrite.

## Publish versions

Each version is a snapshot on the `registry` branch. The version and approver
are metadata:

```python
repo.save_tree(branch="registry", message=f"publish {version}",
               entries={PATH: body, "configs/limits.yaml": "cpu: 1\n"},
               parent=parent, author=velody.Author("release-bot"),
               meta={"registry": {"version": version, "approved_by": "alice"}})
```

The release version lives in metadata, which is part of the snapshot id, so it
cannot be edited afterwards. A release is then found with
`repo.find_snapshots([("registry", "version", "1.1.0")])`. Both bindings also have
tags (`repo.create_tag("v1.1.0", at=id)` in Python, `createTag` in Node), a movable
name you can use alongside the metadata; a tag is not hashed, so the metadata is
the record to trust.

## Roll back

Version 1.2.0 turns out to be bad. Roll back by saving a new snapshot whose
entries carry forward the stored objects of 1.1.0, with
`TreeEntry.stored(path, object, kind)`:

```python
entries = [velody.TreeEntry.stored(f.path, f.object, f.kind)
           for f in repo.tree_at(ids["1.1.0"])]
repo.save_tree(branch="registry", message="rollback to 1.1.0", entries=entries,
               parent=ids["1.2.0"], ...)
```

No file content is read or copied; the new snapshot points at the same objects.
History stays append-only: the bad 1.2.0 is still there, and the script asserts
both that the rolled-back content equals 1.1.0 and that 1.2.0 is intact.

In Node the same rollback passes `{ path, object, kind }` from `treeAt(id)` as the
entries of `saveTree`, with no `data`.

Blame credits the restored lines to the rollback snapshot, not to the original
1.1.0 publish, because a rollback is a new snapshot. To find who approved the
content, read the rollback's metadata (give it `rollback_of` and the approver, as
the script does), or follow the object back to the version that first stored it.

To change one file of a large config tree, `save_changes` sends only that file
and carries the rest forward from the parent:

```python
repo.save_changes(branch="registry", message="publish 1.1.1", parent=parent,
                  changes=[velody.Change.put(PATH, body)],
                  meta={"registry": {"version": "1.1.1", "approved_by": "alice"}})
```

## Audit

`repo.history(branch="registry")` plus `snapshot_meta` gives the full trail:
1.0.0 and 1.1.0 and 1.2.0 approved by alice, then the rollback approved by bob.

For a reviewer without code, `velody report --branch registry -o registry.html`
writes the same trail as a self-contained HTML page.

## Tamper evidence

Metadata is part of the snapshot id. Publishing the same content as 1.2.0 with
`approved_by` set to someone else produces a different id, so the original
approval cannot be swapped out: anyone holding the id of 1.2.0 can verify that
`alice` approved exactly that content. The script asserts the ids differ and
the original metadata is unchanged.
