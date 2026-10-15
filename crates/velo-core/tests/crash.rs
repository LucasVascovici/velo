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
    fsck, history, merge, rebase, redo, restore, save, status, switch, undo,
};
use velo_core::{Error, Repo, SnapshotId};

type Tree = BTreeMap<String, Vec<u8>>;

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
    let scenario = SCENARIOS.iter().find(|s| s.name == op).expect("scenario");
    let repo = Repo::open(&repo_dir).unwrap();
    (scenario.op)(&repo);
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
        .env_remove("VELO_CRASH_AT");
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
    let ctx = format!("{} @ {}", s.name, point);
    let work = scratch.join(format!("run-{point}"));
    copy_dir(template, &work);
    let pre_position = position(&work);
    let log = scratch.join(format!("{point}.log"));
    let out = spawn_child(s.name, &work, &log, Some(point));
    assert!(
        !out.status.success(),
        "{ctx}: child exited 0, so the point was never hit"
    );

    let repo = Repo::open(&work).unwrap();

    // 1. fsck has no problems; repair; then clean.
    let report = fsck::check(&repo).unwrap();
    assert!(
        report.problems.is_empty(),
        "{ctx}: fsck problems: {:?}",
        report.problems
    );
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
            let r = Repo::open(&aborted).unwrap();
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
        let r = Repo::open(&aborted).unwrap();
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
    for s in SCENARIOS {
        let scratch = TempDir::new().unwrap();
        let template = scratch.path().join("template");
        fs::create_dir_all(&template).unwrap();
        velo_core::commands::init::run(&template).unwrap();
        {
            let repo = Repo::open_and_migrate(&template).unwrap();
            (s.build)(&repo);
        }
        let pre = read_tree(&template);

        // The new state, from the same operation run without a handler.
        let twin = scratch.path().join("twin");
        copy_dir(&template, &twin);
        {
            let repo = Repo::open(&twin).unwrap();
            (s.op)(&repo);
        }
        let post = (read_tree(&twin), position(&twin));

        let points = discover(s, &template, scratch.path());
        assert!(!points.is_empty(), "{}: no failpoints were hit", s.name);
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
