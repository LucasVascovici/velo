/* velo.h - C ABI over the embeddable half of velo-core.
 *
 * Conventions
 *   - Every fallible function returns int32_t: 0 on success, otherwise one of
 *     the VELO_ERR_* codes below. The message is velo_last_error_message() on
 *     the same thread.
 *   - Results come back through out-parameters. Strings are NUL-terminated
 *     UTF-8 and are freed with velo_string_free; byte buffers with
 *     velo_bytes_free(data, len).
 *   - Handles are opaque and not thread-safe: one per thread, or serialise.
 *   - Nothing here reads or writes a working tree.
 */
#ifndef VELO_H
#define VELO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Error codes (mirror velo_core::Error, in declaration order) ---- */
#define VELO_OK 0
#define VELO_ERR_NOT_A_REPO 1
#define VELO_ERR_ALREADY_INITIALIZED 2
#define VELO_ERR_NESTED_REPO 3
#define VELO_ERR_SCHEMA_TOO_NEW 4
#define VELO_ERR_MIGRATION_REQUIRED 5
#define VELO_ERR_FORMAT_TOO_OLD 6
#define VELO_ERR_CANCELLED 7
#define VELO_ERR_LOCKED 8
#define VELO_ERR_DIRTY_WORKING_TREE 9
#define VELO_ERR_OPERATION_IN_PROGRESS 10
#define VELO_ERR_NO_OPERATION_IN_PROGRESS 11
#define VELO_ERR_CONFLICTS 12
#define VELO_ERR_DIVERGED 13
#define VELO_ERR_NOT_FAST_FORWARD 14
#define VELO_ERR_UNBORN_BRANCH 15
#define VELO_ERR_NOT_FOUND 16
#define VELO_ERR_AMBIGUOUS_PREFIX 17
#define VELO_ERR_CORRUPT 18
#define VELO_ERR_MISSING_OBJECT 19
#define VELO_ERR_UNTRUSTED_DATA 20
#define VELO_ERR_INVALID_INPUT 21
#define VELO_ERR_UNSUPPORTED 22
#define VELO_ERR_IO 23
#define VELO_ERR_DB 24
/* Snapshot squashed or re-minted by compaction; the message names the live id. */
#define VELO_ERR_COMPACTED 25
/* Binding-level errors. */
#define VELO_ERR_NULL_ARGUMENT 100
#define VELO_ERR_INVALID_UTF8 101
#define VELO_ERR_PANIC 102
#define VELO_ERR_INVALID_JSON 103
#define VELO_ERR_INVALID_OUTPUT 104
/* An error variant newer than this build of the binding. */
#define VELO_ERR_UNKNOWN 255

/* ---- File kinds for velo_tree_add_* ---- */
#define VELO_KIND_REGULAR 0
#define VELO_KIND_EXECUTABLE 1
#define VELO_KIND_SYMLINK 2

typedef struct VeloRepo VeloRepo;
typedef struct VeloTree VeloTree;

/* ---- Errors and memory ---- */

/* Message of the last failure on this thread, or NULL. Owned by the library;
 * valid until the next failing call on the thread. Do not free. */
const char *velo_last_error_message(void);
/* Code of the last failure on this thread, or 0. */
int32_t velo_last_error_code(void);
/* Free a string returned by this library. NULL is a no-op. */
void velo_string_free(char *text);
/* Free a buffer returned by this library with the length it came with. */
void velo_bytes_free(uint8_t *data, size_t len);

/* ---- Repository ---- */

/* Create a repository at path and open it. */
int32_t velo_repo_init(const char *path, VeloRepo **out);
/* Open the repository at path. */
int32_t velo_repo_open(const char *path, VeloRepo **out);
/* Close a repository. NULL is a no-op. */
void velo_repo_free(VeloRepo *repo);
/* Newest snapshot id on branch. An unborn branch is not an error: *out_id is
 * set to NULL and 0 is returned. */
int32_t velo_branch_tip(const VeloRepo *repo, const char *branch, char **out_id);
/* Resolve a tag, branch, id or unique prefix to a full snapshot id. */
int32_t velo_resolve_snapshot(const VeloRepo *repo, const char *spec, char **out_id);

/* ---- Building and saving a tree ---- */

int32_t velo_tree_new(VeloTree **out);
void velo_tree_free(VeloTree *tree);
/* Add a file from data[0..len] (copied). data may be NULL only if len is 0.
 * For VELO_KIND_SYMLINK the data is the link target. */
int32_t velo_tree_add_file(VeloTree *tree, const char *path, const uint8_t *data,
                           size_t len, int32_t kind);
/* Add a file by naming an object already in the store (see velo_tree_at). */
int32_t velo_tree_add_stored(VeloTree *tree, const char *path, const char *object,
                             int32_t kind);
/* Save tree as a new snapshot; writes its id to *out_id.
 *
 * options_json is an object: required "branch" and "message"; optional
 * "parent" and "merge_parent" (any spec), "author" {"name","email"?},
 * "meta" {namespace:{key:value}}, "renames" [[from,to],...], and
 * "timestamp_ms" (integer; omit for now). */
int32_t velo_save_tree(const VeloRepo *repo, const VeloTree *tree,
                       const char *options_json, char **out_id);

/* ---- Reading ---- */

/* JSON array of {"path","object","kind"}; kind is regular|executable|symlink. */
int32_t velo_tree_at(const VeloRepo *repo, const char *spec, char **out_json);
/* File bytes at a snapshot. Free with velo_bytes_free(*out_data, *out_len). */
int32_t velo_read_file_at(const VeloRepo *repo, const char *spec, const char *path,
                          uint8_t **out_data, size_t *out_len);
/* JSON {"id","message","created_at","created_at_ms","branch","parent",
 * "merge_parent","tag"}; absent values are null. */
int32_t velo_snapshot(const VeloRepo *repo, const char *spec, char **out_json);
/* JSON {namespace:{key:value}}, including the reserved "velo" namespace. */
int32_t velo_snapshot_meta(const VeloRepo *repo, const char *spec, char **out_json);

#ifdef __cplusplus
}
#endif

#endif /* VELO_H */
