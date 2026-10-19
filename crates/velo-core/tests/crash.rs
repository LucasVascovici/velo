//! Crash injection: kill the process at every named durable step of an operation
//! and check the repository that is left behind.
//!
//! Self-exec: the parent test builds a fixture, discovers the points an
//! operation hits, then for each point spawns this same test binary with
//! `crash_child` selected. The child installs a handler that aborts when the
//! named point is reached; the parent reopens the damaged copy and checks it.
//!
//! Run with `cargo test -p velo-core --features failpoints --test crash`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use velo_core::commands::{
    bundle, compact, fsck, gc, history, merge, rebase, redo, restore, retention, save, status,
    switch, sync, undo,
};
use velo_core::{Error, Repo, SnapshotId};

type Tree = BTreeMap<String, Vec<u8>>;

thread_local! {
    /// The keyframe interval every repository this thread opens for a scenario
    /// is given; `None` keeps the production default. The child process gets it
    /// through `VELO_CRASH_K`, so parent and child store trees the same way.
    static KEYFRAME_K: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

fn open(path: &Path) -> Repo {
    let mut repo = Repo::open(path).unwrap();
    if let Some(k) = KEYFRAME_K.get() {
        repo.set_keyframe_interval(k);
    }
    repo
}

fn open_migrate(path: &Path) -> Repo {
    let mut repo = Repo::open_and_migrate(path).unwrap();
    if let Some(k) = KEYFRAME_K.get() {
        repo.set_keyframe_interval(k);
    }
    repo
}

/// Runs a scenario's operation on the repository at `path`. A migration
/// scenario opens a v3 repository, which only `open_and_migrate` accepts, and
/// the opening is the operation.
fn run_op(s: &Scenario, path: &Path) {
    if s.kind == Kind::Migrate {
        let _ = open_migrate(path);
    } else {
        (s.op)(&open(path));
    }
}

/// Points that leave a state that is neither the old tree nor the new one, as
/// (point, exact state left). A point listed here is asserted to leave exactly
/// this: `.velo/PARENT` still names the pre-operation snapshot, `status`
/// reports the tree as dirty, and `restore --force` to the snapshot the operation was heading for
/// completes it (restoring the old position would not, because the files the
/// interrupted restore wrote are untracked there).
const KNOWN_WINDOWS: &[(&str, &str)] = &[(
    "restore.after_first_file",
    "a whole-tree restore writes its files one by one, so a kill after the \
     first leaves a mix of old and new files; PARENT still names the old \
     snapshot and status lists the changed files as unsaved; restore --force to the target finishes it",
), (
    "compact.after_commit",
    "the rewrite committed but PARENT was not moved: PARENT names an old id that \
     now resolves to Compacted { into }, the tree is unchanged, and fsck reports \
     exactly one DanglingPosition, and status lists every file as unsaved (nothing is compared against a missing snapshot); restore --force to `into` (same tree, so nothing is lost) fixes it",
), (
    "compact.before_parent",
    "same state as compact.after_commit: the commit is durable, the PARENT rewrite did not happen",
)];

// ── Scenarios ──────────────────────────────────────────────────────────────

struct Scenario {
    name: &'static str,
    build: fn(&Repo),
    op: fn(&Repo),
    kind: Kind,
}

/// What recovery a kill must leave available, beyond the common checks.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    /// A merge: if `MERGE_HEAD` survives, `merge --abort` must put back the
    /// pre-operation tree byte for byte.
    Merge,
    /// An abort of a conflicted merge: re-running the abort must reach the
    /// tree the finished abort leaves.
    MergeAbort,
    /// A rebase: if a rebase state survives, `--abort` must put back the
    /// pre-operation tree and `--continue` must finish or refuse with a typed
    /// error.
    Rebase,
    /// A v3-shaped repository opened with `open_and_migrate`: the kill leaves
    /// a valid v3 or a valid v4, never a mix.
    Migrate,
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn save_all(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn snapshot_by_message(repo: &Repo, msg: &str) -> SnapshotId {
    let h = history::run(
        repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .unwrap();
    h.entries
        .into_iter()
        .find(|e| e.message == msg)
        .map(|e| e.hash)
        .unwrap_or_else(|| panic!("no snapshot '{msg}'"))
}

/// Two snapshots: `one` then `two`, which changes, deletes and adds files.
fn build_two(repo: &Repo) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a one\n");
    write(&root, "b.txt", "b one\n");
    write(&root, "dir/c.txt", "c one\n");
    save_all(repo, "one");
    write(&root, "a.txt", "a two\n");
    fs::remove_file(root.join("b.txt")).unwrap();
    write(&root, "d.txt", "d two\n");
    write(&root, "dir/e.txt", "e two\n");
    save_all(repo, "two");
}

fn build_dirty_save(repo: &Repo) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a one\n");
    write(&root, "b.txt", "b one\n");
    save_all(repo, "one");
    write(&root, "a.txt", "a edited\n");
    fs::remove_file(root.join("b.txt")).unwrap();
    write(&root, "n1.txt", "n1\n");
    write(&root, "sub/n2.txt", "n2\n");
}

fn build_one_file(repo: &Repo) {
    write(repo.root(), "a.txt", "only\n");
    write(repo.root(), "b.txt", "bee\n");
    save_all(repo, "one");
    write(repo.root(), "a.txt", "changed\n");
}

fn build_one_snapshot(repo: &Repo) {
    write(repo.root(), "a.txt", "only\n");
    write(repo.root(), "b.txt", "bee\n");
    save_all(repo, "one");
}

fn build_undone(repo: &Repo) {
    build_two(repo);
    let g = repo.write().unwrap();
    undo::run(&g).unwrap();
}

/// `main` has one snapshot; `feat` has a changed, deleted and added tree; we
/// sit on `main` and the op switches to `feat`.
fn build_branches(repo: &Repo) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a main\n");
    write(&root, "b.txt", "b main\n");
    write(&root, "dir/c.txt", "c main\n");
    save_all(repo, "main one");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "feat", false).unwrap();
    }
    write(&root, "a.txt", "a feat\n");
    fs::remove_file(root.join("b.txt")).unwrap();
    write(&root, "f.txt", "f feat\n");
    write(&root, "dir/g.txt", "g feat\n");
    save_all(repo, "feat one");
    let g = repo.write().unwrap();
    switch::run(&g, "main", false).unwrap();
}

/// `main` and `feat` diverge from one snapshot. `feat` edits `dir/c.txt`, adds
/// `f.txt` and rewrites a CRLF file; `main` edits `a.txt`. With `conflict`,
/// both also edit `shared.txt`. We end on `main`.
fn build_diverged(repo: &Repo, conflict: bool) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a base\n");
    write(&root, "dir/c.txt", "c base\n");
    write(&root, "shared.txt", "one\ntwo\nthree\n");
    write(&root, "crlf.txt", "x\r\ny\r\n");
    save_all(repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "feat", false).unwrap();
    }
    write(&root, "dir/c.txt", "c feat\n");
    write(&root, "f.txt", "f feat\n");
    write(&root, "crlf.txt", "x\r\ny feat\r\n");
    if conflict {
        write(&root, "shared.txt", "one\ntwo feat\nthree\n");
    }
    save_all(repo, "feat one");
    let g = repo.write().unwrap();
    switch::run(&g, "main", false).unwrap();
    drop(g);
    write(&root, "a.txt", "a main\n");
    if conflict {
        write(&root, "shared.txt", "one\ntwo main\nthree\n");
    }
    save_all(repo, "main two");
}

fn build_merge_clean(repo: &Repo) {
    build_diverged(repo, false);
}

fn build_merge_conflict(repo: &Repo) {
    build_diverged(repo, true);
}

fn build_merge_conflicted(repo: &Repo) {
    build_diverged(repo, true);
    op_merge(repo);
}

fn op_merge(r: &Repo) {
    let g = r.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "feat" }).unwrap();
}

fn op_merge_clean_save(r: &Repo) {
    op_merge(r);
    save_all(r, "merged");
}

fn op_merge_abort(r: &Repo) {
    let g = r.write().unwrap();
    merge::run(&g, merge::Mode::Abort).unwrap();
}

/// On `feat`: three snapshots replay onto `main`, which has moved on.
fn build_rebase(repo: &Repo) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a base\n");
    write(&root, "dir/c.txt", "c base\n");
    save_all(repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "feat", false).unwrap();
    }
    write(&root, "r1.txt", "r1\n");
    save_all(repo, "feat one");
    write(&root, "a.txt", "a feat\n");
    write(&root, "dir/r2.txt", "r2\n");
    save_all(repo, "feat two");
    fs::remove_file(root.join("dir/c.txt")).unwrap();
    write(&root, "r3.txt", "r3\n");
    save_all(repo, "feat three");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", false).unwrap();
    }
    write(&root, "m.txt", "m main\n");
    save_all(repo, "main two");
    let g = repo.write().unwrap();
    switch::run(&g, "feat", false).unwrap();
}

fn op_rebase(r: &Repo) {
    let onto = snapshot_by_message(r, "main two");
    let g = r.write().unwrap();
    rebase::run(&g, rebase::Mode::Start { onto: &onto }, None).unwrap();
}

fn op_save(r: &Repo) {
    let g = r.write().unwrap();
    save::run(&g, Some("next"), save::Options::default()).unwrap();
}

fn op_restore(r: &Repo) {
    let id = snapshot_by_message(r, "one");
    let g = r.write().unwrap();
    restore::run(&g, &id, restore::Options::default()).unwrap();
}

fn op_switch(r: &Repo) {
    let g = r.write().unwrap();
    switch::run(&g, "feat", false).unwrap();
}

fn op_undo(r: &Repo) {
    let g = r.write().unwrap();
    undo::run(&g).unwrap();
}

fn op_redo(r: &Repo) {
    let g = r.write().unwrap();
    redo::run(&g).unwrap();
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "save",
        build: build_one_file,
        op: op_save,
        kind: Kind::Plain,
    },
    Scenario {
        name: "save_multi",
        build: build_dirty_save,
        op: op_save,
        kind: Kind::Plain,
    },
    Scenario {
        name: "restore",
        build: build_two,
        op: op_restore,
        kind: Kind::Plain,
    },
    Scenario {
        name: "switch",
        build: build_branches,
        op: op_switch,
        kind: Kind::Plain,
    },
    Scenario {
        name: "undo",
        build: build_two,
        op: op_undo,
        kind: Kind::Plain,
    },
    Scenario {
        name: "undo_root",
        build: build_one_snapshot,
        op: op_undo,
        kind: Kind::Plain,
    },
    Scenario {
        name: "redo",
        build: build_undone,
        op: op_redo,
        kind: Kind::Plain,
    },
    Scenario {
        name: "merge_clean",
        build: build_merge_clean,
        op: op_merge_clean_save,
        kind: Kind::Merge,
    },
    Scenario {
        name: "merge_conflict",
        build: build_merge_conflict,
        op: op_merge,
        kind: Kind::Merge,
    },
    Scenario {
        name: "merge_abort",
        build: build_merge_conflicted,
        op: op_merge_abort,
        kind: Kind::MergeAbort,
    },
    Scenario {
        name: "rebase",
        build: build_rebase,
        op: op_rebase,
        kind: Kind::Rebase,
    },
    Scenario {
        name: "gc",
        build: build_gc,
        op: op_gc,
        kind: Kind::Plain,
    },
    Scenario {
        name: "compact",
        build: build_chain,
        op: op_compact,
        kind: Kind::Plain,
    },
    Scenario {
        name: "retention",
        build: build_chain,
        op: op_retention,
        kind: Kind::Plain,
    },
    Scenario {
        name: "bundle",
        build: build_bundle,
        op: op_bundle,
        kind: Kind::Plain,
    },
    Scenario {
        name: "compact_fold",
        build: build_fold_chain,
        op: op_compact_fold,
        kind: Kind::Plain,
    },
    Scenario {
        name: "migrate_v3",
        build: build_v3,
        op: op_nothing,
        kind: Kind::Migrate,
    },
];

// ── Child ──────────────────────────────────────────────────────────────────

fn handler(name: &'static str) {
    use std::io::Write;
    if let Ok(log) = std::env::var("VELO_CRASH_LOG") {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        // One write call, so concurrent hits from the rayon pool cannot
        // interleave a name with another's newline.
        f.write_all(
            format!(
                "{name}
"
            )
            .as_bytes(),
        )
        .unwrap();
    }
    if std::env::var("VELO_CRASH_AT").as_deref() == Ok(name) {
        std::process::abort();
    }
}

/// Runs only when the parent spawns it with `VELO_CRASH_OP` set.
#[test]
fn crash_child() {
    let Ok(op) = std::env::var("VELO_CRASH_OP") else {
        return;
    };
    let repo_dir = PathBuf::from(std::env::var("VELO_CRASH_REPO").unwrap());
    velo_core::failpoint::set_handler(handler);
    if let Ok(k) = std::env::var("VELO_CRASH_K") {
        KEYFRAME_K.set(Some(k.parse().unwrap()));
    }
    if op == "clone" {
        // The repository is the source; the clone goes to VELO_CRASH_DEST.
        let dest = PathBuf::from(std::env::var("VELO_CRASH_DEST").unwrap());
        clone_into(&repo_dir, &dest).unwrap();
        return;
    }
    let scenario = SCENARIOS.iter().find(|s| s.name == op).expect("scenario");
    run_op(scenario, &repo_dir);
}

// ── Parent helpers ─────────────────────────────────────────────────────────

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dst = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dst);
        } else {
            fs::copy(entry.path(), dst).unwrap();
        }
    }
}

fn read_tree(root: &Path) -> Tree {
    fn walk(root: &Path, dir: &Path, out: &mut Tree) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap();
            if rel == Path::new(".velo") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let key = rel.to_string_lossy().replace('\\', "/");
                out.insert(key, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = Tree::new();
    walk(root, root, &mut out);
    out
}

fn position(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn snapshot_tree(repo: &Repo, id: &str) -> Tree {
    let id: SnapshotId = id.parse().unwrap();
    repo.tree_at(&id)
        .unwrap()
        .into_iter()
        .map(|f| (f.path, repo.read_object(&f.object).unwrap()))
        .collect()
}

fn spawn_child(op: &str, repo: &Path, log: &Path, at: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "crash_child", "--nocapture"])
        .env("VELO_CRASH_OP", op)
        .env("VELO_CRASH_REPO", repo)
        .env("VELO_CRASH_LOG", log)
        .env_remove("VELO_CRASH_AT")
        .env_remove("VELO_CRASH_K");
    if let Some(k) = KEYFRAME_K.get() {
        cmd.env("VELO_CRASH_K", k.to_string());
    }
    if let Some(at) = at {
        cmd.env("VELO_CRASH_AT", at);
    }
    cmd.output().unwrap()
}

/// Points the op hits, in order, without repeats.
fn discover(s: &Scenario, template: &Path, scratch: &Path) -> Vec<String> {
    let work = scratch.join("discover");
    copy_dir(template, &work);
    let log = scratch.join("discover.log");
    let out = spawn_child(s.name, &work, &log, None);
    assert!(
        out.status.success(),
        "{}: discovery run failed: {}",
        s.name,
        String::from_utf8_lossy(&out.stdout)
    );
    let mut seen: Vec<String> = Vec::new();
    for line in fs::read_to_string(&log).unwrap_or_default().lines() {
        if !seen.iter().any(|s| s == line) {
            seen.push(line.to_string());
        }
    }
    seen
}

fn check_point(
    s: &Scenario,
    point: &str,
    template: &Path,
    pre: &Tree,
    post: &(Tree, String),
    scratch: &Path,
) {
    let ctx = format!("{} (K={:?}) @ {}", s.name, KEYFRAME_K.get(), point);
    let work = scratch.join(format!("run-{point}"));
    copy_dir(template, &work);
    let pre_position = position(&work);
    let log = scratch.join(format!("{point}.log"));
    let out = spawn_child(s.name, &work, &log, Some(point));
    assert!(
        !out.status.success(),
        "{ctx}: child exited 0, so the point was never hit"
    );

    let repo = if s.kind == Kind::Migrate {
        after_migration_kill(&ctx, point, &work)
    } else {
        open(&work)
    };

    // 1. fsck has no problems; repair; then clean.
    let report = fsck::check(&repo).unwrap();
    if PARENT_WINDOWS.contains(&point) {
        // Known window: the ids were rewritten but PARENT still names the old
        // one. Exactly that is wrong, and it names the id compaction replaced.
        let shown: Vec<String> = report.problems.iter().map(|p| format!("{p:?}")).collect();
        assert!(
            shown.len() == 1
                && shown[0].contains("DanglingPosition")
                && shown[0].contains(&pre_position),
            "{ctx}: expected only a dangling PARENT, got {shown:?}"
        );
        // The exact state: the tree is untouched and, since PARENT resolves to
        // no snapshot, status lists every tracked file as new.
        assert_same(
            &read_tree(&work),
            pre,
            &format!("{ctx}: the working tree changed"),
        );
        let st = status::run(&repo, &[]).unwrap();
        assert!(
            !st.is_clean(),
            "{ctx}: status is clean with a dangling PARENT"
        );
        let mut listed = st.new_files.clone();
        listed.sort();
        let tracked: Vec<String> = pre.keys().cloned().collect();
        assert_eq!(
            listed, tracked,
            "{ctx}: status must list every file as unsaved"
        );
        // Recovery: the old id says where it went; restore there (same tree).
        let old: SnapshotId = pre_position.parse().unwrap();
        let Err(Error::Compacted { into, .. }) = repo.tree_at(&old) else {
            panic!("{ctx}: the old PARENT does not resolve to Compacted");
        };
        let g = repo.write().unwrap();
        restore::run(
            &g,
            &into,
            restore::Options {
                force: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{ctx}: restoring to {into} failed: {e}"));
        drop(g);
        assert_eq!(position(&work), into.to_string(), "{ctx}: PARENT not fixed");
    } else {
        assert!(
            report.problems.is_empty(),
            "{ctx}: fsck problems: {:?}",
            report.problems
        );
    }
    {
        let g = repo.write().unwrap();
        fsck::repair(&g).unwrap();
    }
    let again = fsck::check(&repo).unwrap();
    assert!(
        again.problems.is_empty() && again.cruft.is_empty(),
        "{ctx}: fsck not clean after repair: {:?} {:?}",
        again.problems,
        again.cruft
    );

    // 2. The tree is the old one or the new one.
    let tree = read_tree(&work);
    let st = status::run(&repo, &[]).unwrap();
    let known = KNOWN_WINDOWS.iter().any(|(p, _)| *p == point);
    // A merge or rebase that was named in `.velo` is an operation in progress:
    // its tree may be anything on the way, and what is asserted instead is that
    // the matching abort puts the old tree back (below).
    let merging = work.join(".velo/MERGE_HEAD").exists();
    let rebasing = work.join(".velo/REBASE_STATE").exists();
    let in_progress = merging || rebasing;
    let mixed = tree != *pre && tree != post.0 && !in_progress;
    if mixed {
        assert!(
            known,
            "{ctx}: working tree is neither the old nor the new one and the point is not a known window"
        );
        // The exact state a known window leaves.
        assert_eq!(position(&work), pre_position, "{ctx}: PARENT moved");
        assert!(!st.is_clean(), "{ctx}: a mixed tree is reported clean");
    }

    // 3. A clean status means the tree is the tree of the snapshot PARENT names.
    if st.is_clean() {
        let pos = position(&work);
        let expect = if pos.is_empty() {
            Tree::new()
        } else {
            snapshot_tree(&repo, &pos)
        };
        assert_same(
            &tree,
            &expect,
            &format!("{ctx}: status is clean but the tree is not PARENT's"),
        );
    }

    // 4. No lock is left behind.
    {
        let g = repo.try_write().unwrap();
        assert!(g.is_some(), "{ctx}: the write lock is still held");
    }

    extra_checks(&ctx, s, point, &repo, &work, template);

    // Known windows are finished by a forced restore to the snapshot the
    // operation was heading for.
    if mixed {
        let g = repo.write().unwrap();
        let target: SnapshotId = post.1.parse().unwrap();
        let force = restore::Options {
            force: true,
            ..Default::default()
        };
        restore::run(&g, &target, force).unwrap();
        drop(g);
        assert_eq!(read_tree(&work), post.0, "{ctx}: finishing did not recover");
    }

    // Recovery of an interrupted merge or rebase.
    if s.kind == Kind::Rebase && rebasing {
        // Abort, on a copy, restores the original head and tree.
        let aborted = scratch.join(format!("abort-{point}"));
        copy_dir(&work, &aborted);
        {
            let r = open(&aborted);
            let g = r.write().unwrap();
            rebase::run(&g, rebase::Mode::Abort, None)
                .unwrap_or_else(|e| panic!("{ctx}: rebase --abort failed: {e}"));
        }
        assert_same(
            &read_tree(&aborted),
            pre,
            &format!("{ctx}: rebase --abort did not restore the pre-rebase tree"),
        );
        assert_eq!(
            position(&aborted),
            pre_position,
            "{ctx}: rebase --abort did not restore PARENT"
        );
        assert!(!aborted.join(".velo/REBASE_STATE").exists());
        assert!(!aborted.join(".velo/MERGE_HEAD").exists());
        let r = open(&aborted);
        let rep = fsck::check(&r).unwrap();
        assert!(
            rep.problems.is_empty(),
            "{ctx}: fsck after rebase --abort: {:?}",
            rep.problems
        );

        // Continue finishes, or refuses with something typed.
        let g = repo.write().unwrap();
        match rebase::run(&g, rebase::Mode::Continue, None) {
            Ok(_) => {
                drop(g);
                assert!(!work.join(".velo/REBASE_STATE").exists());
                assert_eq!(
                    read_tree(&work),
                    post.0,
                    "{ctx}: rebase --continue finished on the wrong tree"
                );
                let h = history::run(
                    &repo,
                    history::Options {
                        all: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                let by_hash: BTreeMap<String, (String, Option<String>)> = h
                    .entries
                    .iter()
                    .map(|e| {
                        (
                            e.hash.to_string(),
                            (e.message.clone(), e.parent.as_ref().map(|p| p.to_string())),
                        )
                    })
                    .collect();
                let mut msgs = Vec::new();
                let mut cur = Some(position(&work));
                while let Some(c) = cur {
                    let Some((m, p)) = by_hash.get(&c) else { break };
                    msgs.push(m.clone());
                    cur = p.clone();
                }
                assert_eq!(
                    msgs[..4],
                    ["feat three", "feat two", "feat one", "main two"],
                    "{ctx}: rebased branch lost or folded a commit"
                );
            }
            Err(e) => assert!(
                !matches!(e, Error::Corrupt { .. }),
                "{ctx}: rebase --continue reported corruption: {e}"
            ),
        }
    } else if s.kind != Kind::Plain && merging && !rebasing {
        // Abort restores the pre-merge tree byte for byte. For a killed abort,
        // that is the tree the finished abort leaves.
        let want = if s.kind == Kind::MergeAbort {
            &post.0
        } else {
            pre
        };
        let want_position = if s.kind == Kind::MergeAbort {
            &post.1
        } else {
            &pre_position
        };
        {
            let g = repo.write().unwrap();
            merge::run(&g, merge::Mode::Abort)
                .unwrap_or_else(|e| panic!("{ctx}: merge --abort failed: {e}"));
        }
        assert_same(
            &read_tree(&work),
            want,
            &format!("{ctx}: merge --abort did not restore the pre-merge tree byte for byte"),
        );
        assert_eq!(position(&work), *want_position, "{ctx}: PARENT moved");
        assert!(!work.join(".velo/MERGE_HEAD").exists());
        assert!(!work.join(".velo/MERGE_ORIG").exists());
        let rep = fsck::check(&repo).unwrap();
        assert!(
            rep.problems.is_empty(),
            "{ctx}: fsck after merge --abort: {:?}",
            rep.problems
        );
    }

    // 5. undo then redo succeed or fail with something typed and not Corrupt.
    for step in 0..2 {
        let g = repo.write().unwrap();
        let r = if step == 0 {
            undo::run(&g).map(|_| ())
        } else {
            redo::run(&g).map(|_| ())
        };
        if let Err(e) = r {
            assert!(
                !matches!(e, Error::Corrupt { .. }),
                "{ctx}: step {step} reported corruption: {e}"
            );
        }
    }
}

#[test]
fn every_point_leaves_old_or_new() {
    if std::env::var("VELO_CRASH_OP").is_ok() {
        return;
    }
    // The production interval (deltas, rare keyframes) and a tiny one, so keyframes
    // and folds are reached by fixtures of a few snapshots. A migration is
    // built from whole trees, so it runs once, at K = 1.
    for (s, k) in SCENARIOS.iter().flat_map(|s| {
        let ks: &[Option<u32>] = if s.kind == Kind::Migrate {
            &[Some(1)]
        } else {
            &[None, Some(3)]
        };
        ks.iter().map(move |k| (s, *k))
    }) {
        KEYFRAME_K.set(k);
        let scratch = TempDir::new().unwrap();
        let template = scratch.path().join("template");
        fs::create_dir_all(&template).unwrap();
        velo_core::commands::init::run(&template).unwrap();
        {
            let repo = open_migrate(&template);
            (s.build)(&repo);
        }
        let pre = read_tree(&template);

        // The new state, from the same operation run without a handler.
        let twin = scratch.path().join("twin");
        copy_dir(&template, &twin);
        run_op(s, &twin);
        let post = (read_tree(&twin), position(&twin));

        let points = discover(s, &template, scratch.path());
        assert!(!points.is_empty(), "{}: no failpoints were hit", s.name);
        for need in required_points(s.name) {
            assert!(
                points.iter().any(|p| p == need),
                "{} (K={k:?}): never hit {need}; hit {points:?}",
                s.name
            );
        }
        for point in &points {
            check_point(s, point, &template, &pre, &post, scratch.path());
        }
    }
}

/// Every window listed has a stated reason, so the table cannot rot into noise.
#[test]
fn known_windows_are_described() {
    for (point, state) in KNOWN_WINDOWS {
        assert!(point.contains('.') && !state.is_empty());
    }
}

/// Compare two trees, naming the paths that differ instead of dumping bytes.
fn assert_same(got: &Tree, want: &Tree, msg: &str) {
    if got == want {
        return;
    }
    let mut diff: Vec<String> = Vec::new();
    for (p, b) in got {
        match want.get(p) {
            None => diff.push(format!("extra {p}")),
            Some(w) if w != b => diff.push(format!("differs {p}")),
            _ => {}
        }
    }
    for p in want.keys().filter(|p| !got.contains_key(*p)) {
        diff.push(format!("missing {p}"));
    }
    panic!("{msg}: {diff:?}");
}

// ── gc, compact, retention, bundle apply and clone ─────────────────────────

/// Deterministic pseudo-random bytes, so a file is stored as chunks (it is
/// over the 1 MiB threshold) and a second version shares most of them.
fn big(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn write_bytes(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Three versions of a chunked file and of a small one. The newest ("three",
/// unsigned) and the one before it ("two", signed) are both undone into the
/// trash. A `gc` with a zero window expires "three" and everything only it
/// refers to, but keeps "two": `gc` never expires the trash entry of a signed
/// snapshot. So every gc point has garbage to delete and a trashed snapshot
/// whose objects must all survive and that `redo` must still bring back.
fn build_gc(repo: &Repo) {
    let root = repo.root().to_path_buf();
    let v1 = big(1, 1_300_000);
    let mut v2 = v1.clone();
    v2[600_000..604_000].copy_from_slice(&big(9, 4_000));
    let mut v3 = v2.clone();
    v3[100_000..104_000].copy_from_slice(&big(7, 4_000));
    write(
        &root, "a.txt", "a one
",
    );
    write_bytes(&root, "big.bin", &v1);
    save_all(repo, "one");
    write(
        &root, "a.txt", "a two
",
    );
    write(
        &root, "n.txt", "n two
",
    );
    write_bytes(&root, "big.bin", &v2);
    save_all(repo, "two");
    write(
        &root, "a.txt", "a three
",
    );
    write(
        &root, "m.txt", "m three
",
    );
    write_bytes(&root, "big.bin", &v3);
    save_all(repo, "three");
    let two = snapshot_by_message(repo, "two");
    let key = velo_core::sign::SigningKey::from_hex(&"11".repeat(32)).unwrap();
    let g = repo.write().unwrap();
    velo_core::sign::sign(&g, &two, &key).unwrap();
    undo::run(&g).unwrap();
    undo::run(&g).unwrap();
}

fn open_db(repo: &Repo) -> rusqlite::Connection {
    rusqlite::Connection::open(repo.root().join(".velo/velo.db")).unwrap()
}

/// Hashes of every snapshot in the trash.
fn trashed_ids(repo: &Repo) -> Vec<String> {
    let db = open_db(repo);
    let mut stmt = db.prepare("SELECT hash FROM trash").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// The files of a trashed snapshot, read back object by object. A trashed
/// snapshot is not a live row, so `tree_at` refuses it; its stored tree (a
/// keyframe in `file_map`, or a delta chain down to one) and the objects it
/// names are what `redo` needs, and what is checked.
fn trashed_tree(repo: &Repo, id: &str) -> Tree {
    let db = open_db(repo);
    // The chain from `id` down to its keyframe, newest first.
    let mut chain = vec![id.to_string()];
    while let Ok(base) = db.query_row(
        "SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?",
        [chain.last().unwrap()],
        |r| r.get::<_, String>(0),
    ) {
        chain.push(base);
    }
    chain.reverse();
    let mut stmt = db
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
        .unwrap();
    let mut map: BTreeMap<String, velo_core::ObjectHash> = stmt
        .query_map([&chain[0]], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut delta = db
        .prepare("SELECT path, hash FROM tree_delta_rows WHERE snapshot_hash = ?")
        .unwrap();
    for link in &chain[1..] {
        let rows: Vec<(String, Option<velo_core::ObjectHash>)> = delta
            .query_map([link], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for (path, hash) in rows {
            match hash {
                Some(h) => map.insert(path, h),
                None => map.remove(&path),
            };
        }
    }
    let rows: Vec<(String, velo_core::ObjectHash)> = map.into_iter().collect();
    assert!(!rows.is_empty(), "trashed snapshot {id} lost its file_map");
    rows.into_iter()
        .map(|(path, hash)| {
            let obj = repo
                .read_object(&hash)
                .unwrap_or_else(|e| panic!("trashed snapshot {id}: object of {path}: {e}"));
            (path, obj)
        })
        .collect()
}

fn op_gc(r: &Repo) {
    let g = r.write().unwrap();
    gc::run(
        &g,
        gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    )
    .unwrap();
}

/// Five snapshots on `main`, each changing one file and adding one.
fn build_chain(repo: &Repo) {
    let root = repo.root().to_path_buf();
    for i in 1..=5 {
        write(&root, "a.txt", &format!("a {i}\n"));
        write(&root, &format!("f{i}.txt"), &format!("f{i}\n"));
        save_all(repo, &format!("s{i}"));
    }
}

/// Six snapshots, so that at K = 3 the removed range (`s2`..`s4`) has a delta
/// dependant (`s5` is based on `s4`) and compaction must fold it.
fn build_fold_chain(repo: &Repo) {
    let root = repo.root().to_path_buf();
    for i in 1..=6 {
        write(
            &root,
            "a.txt",
            &format!(
                "a {i}
"
            ),
        );
        write(
            &root,
            &format!("f{i}.txt"),
            &format!(
                "f{i}
"
            ),
        );
        save_all(repo, &format!("s{i}"));
    }
}

fn op_compact_fold(r: &Repo) {
    let oldest = snapshot_by_message(r, "s2");
    let newest = snapshot_by_message(r, "s3");
    let g = r.write().unwrap();
    let branch = "main".parse().unwrap();
    compact::run(
        &g,
        &branch,
        &[compact::Range {
            oldest: &oldest,
            newest: &newest,
        }],
        Default::default(),
    )
    .unwrap();
}

/// A v3-shaped repository: snapshots stored as whole trees, no delta tables,
/// `user_version` 3. Built as 20.3-b's migration test builds one.
fn build_v3(repo: &Repo) {
    let root = repo.root().to_path_buf();
    for i in 1..=4 {
        write(
            &root, "keep.txt", "same
",
        );
        write(
            &root,
            &format!("f{i}.txt"),
            &format!(
                "f{i}
"
            ),
        );
        save_all(repo, &format!("s{i}"));
    }
    open_db(repo)
        .execute_batch(
            "DROP TABLE tree_deltas; DROP TABLE tree_delta_rows; PRAGMA user_version = 3;",
        )
        .unwrap();
}

fn op_nothing(_: &Repo) {}

/// The repository after a migration was killed: still a complete v3 when the
/// kill came before the stamp, and a complete v4 once the transaction
/// committed. A half-migrated repository (tables present, version 3, or the
/// reverse) is neither, and fails here.
fn after_migration_kill(ctx: &str, point: &str, work: &Path) -> Repo {
    match Repo::open(work) {
        Ok(repo) => {
            assert_ne!(
                point, "migrate.before_stamp",
                "{ctx}: the migration committed although it was killed before the stamp"
            );
            repo
        }
        Err(Error::MigrationRequired { found: 3, .. }) => {
            let db = rusqlite::Connection::open(work.join(".velo/velo.db")).unwrap();
            let tables: i64 = db
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name IN ('tree_deltas', 'tree_delta_rows')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tables, 0, "{ctx}: a v3 repository has v4 tables");
            let ok: String = db
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .unwrap();
            assert_eq!(ok, "ok", "{ctx}: the v3 database is damaged");
            drop(db);
            // It still migrates, and the result is a v4 repository.
            open_migrate(work)
        }
        Err(e) => panic!("{ctx}: reopening after a migration kill: {e}"),
    }
}

fn op_compact(r: &Repo) {
    let oldest = snapshot_by_message(r, "s2");
    let newest = snapshot_by_message(r, "s3");
    let g = r.write().unwrap();
    let branch = "main".parse().unwrap();
    compact::run(
        &g,
        &branch,
        &[compact::Range {
            oldest: &oldest,
            newest: &newest,
        }],
        Default::default(),
    )
    .unwrap();
}

fn op_retention(r: &Repo) {
    const DAY: i64 = 86_400_000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let policy = retention::Policy {
        tiers: vec![retention::Tier {
            older_than_ms: DAY,
            keep_one_per_ms: DAY,
        }],
    };
    let g = r.write().unwrap();
    let branch = "main".parse().unwrap();
    let report = retention::apply(
        &g,
        &branch,
        &policy,
        retention::Options {
            now_ms: Some(now + 10 * DAY),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.removed > 0, "retention squashed nothing");
}

/// The bundle comes from a separate source repository built next to the
/// template (every scenario copy lives in one scratch directory), so each copy
/// of the destination finds it at the same relative place.
fn bundle_file(repo: &Repo) -> PathBuf {
    repo.root().parent().unwrap().join("in.velob")
}

fn build_bundle(repo: &Repo) {
    let src = repo.root().parent().unwrap().join("bundle-src");
    fs::create_dir_all(&src).unwrap();
    velo_core::commands::init::run(&src).unwrap();
    let source = open_migrate(&src);
    build_bundle_source(&source);
    bundle::create(&source, &bundle_file(repo), None).unwrap();
}

/// Three snapshots with a chunked file, nothing in the trash.
fn build_bundle_source(repo: &Repo) {
    let root = repo.root().to_path_buf();
    write(&root, "a.txt", "a one\n");
    write_bytes(&root, "big.bin", &big(1, 1_300_000));
    save_all(repo, "one");
    write(&root, "a.txt", "a two\n");
    write(&root, "dir/n.txt", "n two\n");
    save_all(repo, "two");
    write_bytes(&root, "big.bin", &big(2, 1_100_000));
    save_all(repo, "three");
}

fn op_bundle(r: &Repo) {
    let g = r.write().unwrap();
    bundle::apply(&g, &bundle_file(r)).unwrap();
}

/// Points a scenario must hit, so a failpoint that stops being reached (a
/// refactor moves it, a fixture stops exercising it) fails the test instead of
/// silently shrinking the coverage.
fn required_points(name: &str) -> &'static [&'static str] {
    match name {
        "gc" => &["gc.after_prune", "gc.after_object", "gc.after_chunk"],
        "compact_fold" => &[
            "compact.before_commit",
            "compact.after_commit",
            "compact.before_parent",
            "tree_store.fold.after_rewrite",
        ],
        "compact" | "retention" => &[
            "compact.before_commit",
            "compact.after_commit",
            "compact.before_parent",
        ],
        "migrate_v3" => &["migrate.before_stamp"],
        "bundle" => &[
            "bundle.after_objects",
            "bundle.before_commit",
            "bundle.after_commit",
        ],
        _ => &[],
    }
}

fn live_ids(repo: &Repo) -> Vec<String> {
    history::run(
        repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|e| e.hash.to_string())
    .collect()
}

/// The checks that belong to one operation, run on the crashed repository.
fn extra_checks(ctx: &str, s: &Scenario, point: &str, repo: &Repo, work: &Path, template: &Path) {
    match s.name {
        "gc" => {
            // No object a live snapshot refers to is missing: every file of every
            // live snapshot reads back, chunked ones included.
            for id in live_ids(repo) {
                let _ = snapshot_tree(repo, &id);
            }
            // Nor one a trashed snapshot refers to: the signed one survives
            // every gc point whole, and redo brings it back (on a copy, so the
            // crashed repository is left as the rest of the checks expect).
            let trashed = trashed_ids(repo);
            assert_eq!(trashed.len(), 1, "{ctx}: trash is {trashed:?}");
            let want = trashed_tree(repo, &trashed[0]);
            let copy = work.parent().unwrap().join(format!("redo-{point}"));
            copy_dir(work, &copy);
            {
                let r = open(&copy);
                let g = r.write().unwrap();
                let out = redo::run(&g)
                    .unwrap_or_else(|e| panic!("{ctx}: redo of the trashed snapshot: {e}"));
                assert_eq!(out.snapshot, trashed[0], "{ctx}: redo brought back another");
                drop(g);
                assert_same(
                    &read_tree(&copy),
                    &want,
                    &format!("{ctx}: redo did not restore the trashed snapshot's files"),
                );
            }
            // A second pass finishes what the first left, and breaks nothing.
            op_gc(repo);
            for id in live_ids(repo) {
                let _ = snapshot_tree(repo, &id);
            }
            assert_same(
                &trashed_tree(repo, &trashed[0]),
                &want,
                &format!("{ctx}: a second gc damaged the trashed snapshot"),
            );
            let rep = fsck::check(repo).unwrap();
            assert!(
                rep.problems.is_empty() && rep.cruft.is_empty(),
                "{ctx}: fsck after a second gc: {:?} {:?}",
                rep.problems,
                rep.cruft
            );
        }
        "compact" | "retention" | "compact_fold" => {
            let old = live_ids(&open(template));
            let live = live_ids(repo);
            let committed = matches!(point, "compact.after_commit" | "compact.before_parent");
            let mut gone = 0;
            for id in &old {
                let sid: SnapshotId = id.parse().unwrap();
                match repo.tree_at(&sid) {
                    Ok(_) => assert!(live.contains(id), "{ctx}: {id} resolves but is not live"),
                    Err(Error::Compacted { into, .. }) => {
                        gone += 1;
                        assert!(!live.contains(id), "{ctx}: {id} is both live and compacted");
                        assert!(
                            live.contains(&into.to_string()),
                            "{ctx}: {id} compacted into {into}, which is not live"
                        );
                    }
                    Err(e) => panic!("{ctx}: old id {id}: {e}"),
                }
            }
            assert_eq!(
                gone > 0,
                committed,
                "{ctx}: an old id resolves to Compacted only if the rewrite committed"
            );
            if !committed {
                assert_eq!(
                    live.len(),
                    old.len(),
                    "{ctx}: a rolled-back rewrite changed history"
                );
            }
        }
        "bundle" => {
            // Re-applying completes the import, whatever the crash left.
            let src = open(&work.parent().unwrap().join("bundle-src"));
            let want = live_ids(&src);
            {
                let g = repo.write().unwrap();
                bundle::apply(&g, &bundle_file(repo))
                    .unwrap_or_else(|e| panic!("{ctx}: re-applying the bundle failed: {e}"));
            }
            let mut got = live_ids(repo);
            let mut want_sorted = want.clone();
            got.sort();
            want_sorted.sort();
            assert_eq!(
                got, want_sorted,
                "{ctx}: re-apply did not yield the full import"
            );
            for id in &want {
                assert_eq!(
                    snapshot_tree(repo, id),
                    snapshot_tree(&src, id),
                    "{ctx}: imported tree of {id} differs"
                );
            }
            let rep = fsck::check(repo).unwrap();
            assert!(
                rep.problems.is_empty() && rep.cruft.is_empty(),
                "{ctx}: fsck after re-apply: {:?} {:?}",
                rep.problems,
                rep.cruft
            );
            // Idempotent: another apply imports nothing.
            let g = repo.write().unwrap();
            let again = bundle::apply(&g, &bundle_file(repo)).unwrap();
            assert!(
                again.is_empty(),
                "{ctx}: a second re-apply imported something"
            );
        }
        _ => {}
    }
}

/// Points where `compact` has committed the new ids but not yet moved
/// `.velo/PARENT`, so PARENT names an id that no longer exists.
const PARENT_WINDOWS: &[&str] = &["compact.after_commit", "compact.before_parent"];

/// Clone points before the checkout and the exact state each leaves: (point,
/// whether the snapshots were imported, state). Every clone kill leaves a
/// directory holding a `.velo`, so a second clone refuses it; the recovery is to
/// delete the directory and clone again.
const CLONE_WINDOWS: &[(&str, bool, &str)] = &[
    (
        "clone.after_init",
        false,
        "an empty repository: no snapshots, no files, no origin remote",
    ),
    (
        "tree_store.after_keyframe_rows",
        false,
        "inside the import transaction, which was never committed: same as bundle.before_commit",
    ),
    (
        "tree_store.after_delta_rows",
        false,
        "inside the import transaction, which was never committed: same as bundle.before_commit",
    ),
    (
        "bundle.after_objects",
        false,
        "the objects are stored but no snapshot row exists: no snapshots, no files; the objects are unreferenced",
    ),
    (
        "bundle.before_commit",
        false,
        "same as bundle.after_objects: the import transaction was never committed",
    ),
    (
        "bundle.after_commit",
        true,
        "all snapshots imported, no origin remote row, nothing checked out",
    ),
    (
        "clone.after_import",
        true,
        "all snapshots imported, no origin remote row, nothing checked out",
    ),
];

fn spawn_clone_child(
    src: &Path,
    dest: &Path,
    log: &Path,
    at: Option<&str>,
) -> std::process::Output {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "crash_child", "--nocapture"])
        .env("VELO_CRASH_OP", "clone")
        .env("VELO_CRASH_REPO", src)
        .env("VELO_CRASH_DEST", dest)
        .env("VELO_CRASH_LOG", log)
        .env_remove("VELO_CRASH_AT");
    if let Some(at) = at {
        cmd.env("VELO_CRASH_AT", at);
    }
    cmd.output().unwrap()
}

fn clone_into(src: &Path, dest: &Path) -> velo_core::Result<sync::Cloned> {
    let spawn = velo_core::transport::Spawn::new(std::env::current_exe().unwrap());
    sync::clone(
        &src.to_string_lossy(),
        &spawn,
        sync::CloneOptions {
            dir: Some(dest),
            ..Default::default()
        },
    )
}

#[test]
fn clone_survives_or_is_a_listed_window() {
    if std::env::var("VELO_CRASH_OP").is_ok() {
        return;
    }
    let scratch = TempDir::new().unwrap();
    let src = scratch.path().join("src");
    fs::create_dir_all(&src).unwrap();
    velo_core::commands::init::run(&src).unwrap();
    {
        let repo = Repo::open_and_migrate(&src).unwrap();
        build_two(&repo);
        write(&src, "more/x.txt", "x\n");
        write(&src, "more/y.txt", "y\n");
        save_all(&repo, "three");
    }
    let source_tree = read_tree(&src);
    let source_ids = {
        let mut v = live_ids(&Repo::open(&src).unwrap());
        v.sort();
        v
    };

    let log = scratch.path().join("discover.log");
    let out = spawn_clone_child(&src, &scratch.path().join("d0"), &log, None);
    assert!(out.status.success(), "clone discovery failed");
    assert_eq!(read_tree(&scratch.path().join("d0")), source_tree);
    let mut points: Vec<String> = Vec::new();
    for line in fs::read_to_string(&log).unwrap().lines() {
        if !points.iter().any(|p| p == line) {
            points.push(line.to_string());
        }
    }
    for need in [
        "clone.after_init",
        "clone.after_import",
        "restore.after_first_file",
    ] {
        assert!(points.iter().any(|p| p == need), "clone never hit {need}");
    }

    for point in &points {
        let ctx = format!("clone @ {point}");
        let dest = scratch.path().join(format!("run-{point}"));
        let log = scratch.path().join(format!("{point}.log"));
        let out = spawn_clone_child(&src, &dest, &log, Some(point));
        assert!(!out.status.success(), "{ctx}: child exited 0");

        // The exact state: a repository directory that opens and has the
        // snapshots it had when the kill came.
        assert!(dest.join(".velo").is_dir(), "{ctx}: no repository left");
        let repo = Repo::open(&dest).unwrap();
        let ids = {
            let mut v = live_ids(&repo);
            v.sort();
            v
        };
        let mut tree = read_tree(&dest);
        tree.remove(".veloignore"); // written by init, not part of the checkout
        match CLONE_WINDOWS.iter().find(|(p, _, _)| p == point) {
            Some((_, imported, _)) => {
                if *imported {
                    assert_eq!(ids, source_ids, "{ctx}: snapshots missing");
                } else {
                    assert!(ids.is_empty(), "{ctx}: snapshots before the import");
                }
                assert!(
                    tree.is_empty(),
                    "{ctx}: files before the checkout: {:?}",
                    tree.keys()
                );
            }
            None => {
                // Inside the checkout: every snapshot is in, and each file that
                // exists is the source's (restore writes each file whole).
                assert_eq!(ids, source_ids, "{ctx}: snapshots missing");
                // Restore's workers race: when the first file's point aborts
                // the process, another worker may be inside `fs::write` of its
                // own file, which leaves a prefix of the source's bytes (often
                // none). That one file is the only allowed deviation.
                for (p, b) in &tree {
                    let want = source_tree
                        .get(p)
                        .unwrap_or_else(|| panic!("{ctx}: {p} is not in the source"));
                    if b != want {
                        assert!(
                            point == "restore.after_first_file" && want.starts_with(b),
                            "{ctx}: {p} is not the source's"
                        );
                    }
                }
                if point == "restore.after_first_file" {
                    assert!(
                        tree.len() < source_tree.len(),
                        "{ctx}: the checkout had finished"
                    );
                }
            }
        }
        assert!(
            repo.try_write().unwrap().is_some(),
            "{ctx}: the write lock is still held"
        );
        drop(repo);

        // A second clone refuses the directory; delete it and clone again.
        let err = clone_into(&src, &dest).expect_err("clone into a leftover repository");
        assert!(
            err.to_string()
                .contains("already contains a Velo repository"),
            "{ctx}: {err}"
        );
        fs::remove_dir_all(&dest).unwrap();
        clone_into(&src, &dest).unwrap_or_else(|e| panic!("{ctx}: re-clone failed: {e}"));
        assert_eq!(read_tree(&dest), source_tree, "{ctx}: re-clone differs");
        let repo = Repo::open(&dest).unwrap();
        assert!(fsck::check(&repo).unwrap().problems.is_empty());
    }
}
