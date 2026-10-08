# velo-ffi

A C ABI over the embeddable half of `velo-core`: opaque handles, explicit free
functions, and errors as a code plus a message. Nothing here touches a working
tree.

Build it with `cargo build -p velo-ffi --release`; the `cdylib` and `staticlib`
land in `target/release/`, and the declarations are in `include/velo.h`. This
crate is not published to crates.io.

## Conventions

- Every fallible function returns `int32_t`: `0` on success, otherwise a code.
  `velo_last_error_message()` returns the text for the calling thread.
- Results use out-parameters. Free strings with `velo_string_free` and byte
  buffers with `velo_bytes_free(data, len)`.
- Handles (`VeloRepo`, `VeloTree`) are not thread-safe: one per thread.
- Structured results are JSON.
- Panics are caught and returned as `VELO_ERR_PANIC`.

## Error codes

| Code | Name | Meaning |
| ---: | --- | --- |
| 0 | `VELO_OK` | Success |
| 1 | `VELO_ERR_NOT_A_REPO` | No repository at the path |
| 2 | `VELO_ERR_ALREADY_INITIALIZED` | A repository already exists |
| 3 | `VELO_ERR_NESTED_REPO` | Inside an existing repository |
| 4 | `VELO_ERR_SCHEMA_TOO_NEW` | Written by a newer Velo |
| 5 | `VELO_ERR_MIGRATION_REQUIRED` | Needs migrating first |
| 6 | `VELO_ERR_FORMAT_TOO_OLD` | Too old to upgrade in place |
| 7 | `VELO_ERR_CANCELLED` | Cancelled |
| 8 | `VELO_ERR_LOCKED` | Another process holds the write lock |
| 9 | `VELO_ERR_DIRTY_WORKING_TREE` | Would overwrite unsaved changes |
| 10 | `VELO_ERR_OPERATION_IN_PROGRESS` | Merge, rebase or cherry-pick underway |
| 11 | `VELO_ERR_NO_OPERATION_IN_PROGRESS` | None underway |
| 12 | `VELO_ERR_CONFLICTS` | Conflicts need resolving |
| 13 | `VELO_ERR_DIVERGED` | Local and remote both advanced |
| 14 | `VELO_ERR_NOT_FAST_FORWARD` | Push would discard history |
| 15 | `VELO_ERR_UNBORN_BRANCH` | Branch has no snapshots |
| 16 | `VELO_ERR_NOT_FOUND` | A reference did not resolve |
| 17 | `VELO_ERR_AMBIGUOUS_PREFIX` | Prefix matched several snapshots |
| 18 | `VELO_ERR_CORRUPT` | Stored data failed verification |
| 19 | `VELO_ERR_MISSING_OBJECT` | Object absent from the store |
| 20 | `VELO_ERR_UNTRUSTED_DATA` | Received data failed verification |
| 21 | `VELO_ERR_INVALID_INPUT` | The request was invalid |
| 22 | `VELO_ERR_UNSUPPORTED` | Not supported here |
| 23 | `VELO_ERR_IO` | I/O error |
| 24 | `VELO_ERR_DB` | Database error |
| 25 | `VELO_ERR_COMPACTED` | Snapshot squashed by compaction; the message names the live id |
| 100 | `VELO_ERR_NULL_ARGUMENT` | A required pointer was NULL |
| 101 | `VELO_ERR_INVALID_UTF8` | A string was not UTF-8 |
| 102 | `VELO_ERR_PANIC` | Rust panicked inside the call |
| 103 | `VELO_ERR_INVALID_JSON` | A JSON argument was malformed |
| 104 | `VELO_ERR_INVALID_OUTPUT` | The result could not be returned as a C string |
| 255 | `VELO_ERR_UNKNOWN` | An error newer than this binding |

## Saving a tree

```c
VeloRepo *repo; VeloTree *tree; char *id;
velo_repo_init("/tmp/r", &repo);
velo_tree_new(&tree);
velo_tree_add_file(tree, "a.txt", (const uint8_t *)"hi\n", 3, VELO_KIND_REGULAR);
if (velo_save_tree(repo, tree,
        "{\"branch\":\"main\",\"message\":\"first\"}", &id) != 0)
    fprintf(stderr, "%s\n", velo_last_error_message());
velo_string_free(id);
velo_tree_free(tree);
velo_repo_free(repo);
```

## History and blame

`velo_history(repo, options_json, &out)` takes `from`, `branch`, `all`, `paths`,
`limit` and `meta` (`[{"namespace","key","value"|null}]`) and returns
`{"entries": [...], "empty": reason|null}`. `velo_find_snapshots` takes just the
filter array. `velo_blame(repo, path, options_json, &out)` returns every line with
its `line_count` and an `origin` (id, author, branch); `start_line`/`end_line` are
1-based and inclusive.

**Pass `from` or `branch` to `velo_history`.** With neither (and no `all`) it
follows `.velo/PARENT`, which an embedder working on the store alone does not have.

## Merge

`velo_merge_base` writes NULL when two snapshots share no history.
`velo_merge_plan` lists each file with its `action`; conflicted files carry the
`base`, `ours` and `theirs` object ids. `velo_merge_commit` records the merge from
a spec naming `branch`, `ours`, `theirs`, `message` and per-path `resolutions`
(`"ours"`, `"theirs"`, `"delete"` or `{"content_base64": ...}`); an unresolved
conflict returns `VELO_ERR_CONFLICTS` with the paths in the message.

## Branches

`velo_branches` lists branches with their tips, `velo_branch_create` makes one at a
snapshot (or the current one when NULL) and `velo_branch_set_tip` moves it; an
unknown snapshot is `VELO_ERR_NOT_FOUND`.
