# velody-ffi

A C ABI over the embeddable half of `velody-core`: opaque handles, explicit free
functions, and errors as a code plus a message. Nothing here touches a working
tree.

Build it with `cargo build -p velody-ffi --release`; the `cdylib` and `staticlib`
land in `target/release/`, and the declarations are in `include/velody.h`. This
crate is not published to crates.io.

## Conventions

- Every fallible function returns `int32_t`: `0` on success, otherwise a code.
  `velody_last_error_message()` returns the text for the calling thread.
- Results use out-parameters. Free strings with `velody_string_free` and byte
  buffers with `velody_bytes_free(data, len)`.
- Handles (`VelodyRepo`, `VelodyTree`) are not thread-safe: one per thread.
- Structured results are JSON.
- Panics are caught and returned as `VELODY_ERR_PANIC`.

## Error codes

| Code | Name | Meaning |
| ---: | --- | --- |
| 0 | `VELODY_OK` | Success |
| 1 | `VELODY_ERR_NOT_A_REPO` | No repository at the path |
| 2 | `VELODY_ERR_ALREADY_INITIALIZED` | A repository already exists |
| 3 | `VELODY_ERR_NESTED_REPO` | Inside an existing repository |
| 4 | `VELODY_ERR_SCHEMA_TOO_NEW` | Written by a newer Velody |
| 5 | `VELODY_ERR_MIGRATION_REQUIRED` | Needs migrating first |
| 6 | `VELODY_ERR_FORMAT_TOO_OLD` | Too old to upgrade in place |
| 7 | `VELODY_ERR_CANCELLED` | Cancelled |
| 8 | `VELODY_ERR_LOCKED` | Another process holds the write lock |
| 9 | `VELODY_ERR_DIRTY_WORKING_TREE` | Would overwrite unsaved changes |
| 10 | `VELODY_ERR_OPERATION_IN_PROGRESS` | Merge, rebase or cherry-pick underway |
| 11 | `VELODY_ERR_NO_OPERATION_IN_PROGRESS` | None underway |
| 12 | `VELODY_ERR_CONFLICTS` | Conflicts need resolving |
| 13 | `VELODY_ERR_DIVERGED` | Local and remote both advanced |
| 14 | `VELODY_ERR_NOT_FAST_FORWARD` | Push would discard history |
| 15 | `VELODY_ERR_UNBORN_BRANCH` | Branch has no snapshots |
| 16 | `VELODY_ERR_NOT_FOUND` | A reference did not resolve |
| 17 | `VELODY_ERR_AMBIGUOUS_PREFIX` | Prefix matched several snapshots |
| 18 | `VELODY_ERR_CORRUPT` | Stored data failed verification |
| 19 | `VELODY_ERR_MISSING_OBJECT` | Object absent from the store |
| 20 | `VELODY_ERR_UNTRUSTED_DATA` | Received data failed verification |
| 21 | `VELODY_ERR_INVALID_INPUT` | The request was invalid |
| 22 | `VELODY_ERR_UNSUPPORTED` | Not supported here |
| 23 | `VELODY_ERR_IO` | I/O error |
| 24 | `VELODY_ERR_DB` | Database error |
| 25 | `VELODY_ERR_COMPACTED` | Snapshot squashed by compaction; the message names the live id |
| 26 | `VELODY_ERR_UNSAFE_PATH` | A tree or bundle holds a path velody refuses to store or write; the message names it |
| 27 | `VELODY_ERR_LIMIT_EXCEEDED` | Received data would exceed a read limit (object, pack or string size); nothing was imported |
| 28 | `VELODY_ERR_ACCESS_DENIED` | A remote refused access (missing or invalid bearer token, or a read-only server) |
| 29 | `VELODY_ERR_WOULD_OVERWRITE_EDITS` | Taking a side would replace edits made by hand to conflicted files |
| 30 | `VELODY_ERR_MISSING_CLEAN_CHANGES` | A hand edit would drop lines the other side changed cleanly |
| 31 | `VELODY_ERR_SNAPSHOT_HAS_DEPENDENTS` | Undoing a snapshot would leave other snapshots without their parent |
| 32 | `VELODY_ERR_REMOTE_REF_ABSENT` | A remote-tracking ref names a snapshot this repository does not hold; fetch to restore it |
| 33 | `VELODY_ERR_REDO_BRANCH_MOVED` | `redo` refused: the branch moved after the undo, so the undone snapshot is no longer its child |
| 100 | `VELODY_ERR_NULL_ARGUMENT` | A required pointer was NULL |
| 101 | `VELODY_ERR_INVALID_UTF8` | A string was not UTF-8 |
| 102 | `VELODY_ERR_PANIC` | Rust panicked inside the call |
| 103 | `VELODY_ERR_INVALID_JSON` | A JSON argument was malformed |
| 104 | `VELODY_ERR_INVALID_OUTPUT` | The result could not be returned as a C string |
| 255 | `VELODY_ERR_UNKNOWN` | An error newer than this binding |

## Saving a tree

```c
VelodyRepo *repo; VelodyTree *tree; char *id;
velody_repo_init("/tmp/r", &repo);
velody_tree_new(&tree);
velody_tree_add_file(tree, "a.txt", (const uint8_t *)"hi\n", 3, VELODY_KIND_REGULAR);
if (velody_save_tree(repo, tree,
        "{\"branch\":\"main\",\"message\":\"first\"}", &id) != 0)
    fprintf(stderr, "%s\n", velody_last_error_message());
velody_string_free(id);
velody_tree_free(tree);
velody_repo_free(repo);
```

## History and blame

`velody_history(repo, options_json, &out)` takes `from`, `branch`, `all`, `paths`,
`limit` and `meta` (`[{"namespace","key","value"|null}]`) and returns
`{"entries": [...], "empty": reason|null}`. `velody_find_snapshots` takes just the
filter array. `velody_blame(repo, path, options_json, &out)` returns every line with
its `line_count` and an `origin` (id, author, branch); `start_line`/`end_line` are
1-based and inclusive.

**Pass `from` or `branch` to `velody_history`.** With neither (and no `all`) it
follows `.velo/PARENT`, which an embedder working on the store alone does not have.

## Merge

`velody_merge_base` writes NULL when two snapshots share no history.
`velody_merge_plan` lists each file with its `action`; conflicted files carry the
`base`, `ours` and `theirs` object ids. `velody_merge_commit` records the merge from
a spec naming `branch`, `ours`, `theirs`, `message` and per-path `resolutions`
(`"ours"`, `"theirs"`, `"delete"` or `{"content_base64": ...}`); an unresolved
conflict returns `VELODY_ERR_CONFLICTS` with the paths in the message.

## Branches

`velody_branches` lists branches with their tips, `velody_branch_create` makes one at a
snapshot (or the current one when NULL) and `velody_branch_set_tip` moves it; an
unknown snapshot is `VELODY_ERR_NOT_FOUND`.
