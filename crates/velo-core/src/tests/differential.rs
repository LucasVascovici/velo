//! Seeded differential harness (phase 20.1-b): the same random sequence of
//! operations drives a reference repository and a subject repository, and after
//! **every** operation the two must agree on snapshot ids, the tree at every
//! live snapshot, history, blame and `status`, and both must pass `fsck`.
//!
//! Today both sides run the same engine, so the harness passes against itself;
//! that is the point of landing it first. Later tasks give the subject a
//! different storage configuration (see [`Config`]) and this file is what proves
//! the change is invisible.
//!
//! Everything is replayable: an op list is a pure function of `(seed, count,
//! mix)`, ops name their targets by index and are resolved against the live
//! state when they run, and the clock is pinned per handle. A failure prints the
//! seed and the shortest op list that still fails, as a `Vec<Op>` literal.
//!
//! Knobs, read here and nowhere in the library: `VELO_DIFF_SEED`,
//! `VELO_DIFF_OPS` (default 40) and `VELO_DIFF_SECS` (the `#[ignore]`d long run).
//!
//! Symlink entries are not generated: they could only enter through `save_tree`
//! and only on Unix, which would make the two platforms' op lists differ.

use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::perf_support::Rng;
use crate::commands::{
    blame, branches, cherry_pick, compact, fsck, gc, history, merge, rebase, redo, remote,
    retention, save, squash, stash, status, switch, sync, tag, undo,
};
use crate::transport::Spawn;
use crate::tree::{SaveChanges, SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, SnapshotId, TagName};

/// How one side is built. `keyframe_interval` is the side's K (`None`: the
/// default), set on every repository handle it opens.
#[derive(Clone, Debug, Default)]
struct Config {
    keyframe_interval: Option<u32>,
}

/// Which operations the generator may draw. `removals` gates the ones that
/// delete or re-key snapshots (squash, compact, retention, gc), so a later task
/// can switch them off while its storage cannot yet fold them.
#[derive(Clone, Copy, Debug)]
struct Mix {
    removals: bool,
    /// Fetch, push and remote commits; off until 20.4-b makes import write
    /// deltas.
    network: bool,
}

/// One side of the comparison. `a` is the repository under test; `b` is its
/// `origin` (a path remote), so fetch and push have somewhere to go.
struct Side {
    a: Repo,
    b: Repo,
    _dirs: TempDir,
}

/// The single constructor for a side; `cfg.keyframe_interval` is applied here.
fn side(cfg: &Config) -> Side {
    let dirs = TempDir::new().unwrap();
    let mut repos = Vec::new();
    for name in ["a", "b"] {
        let root = dirs.path().join(name);
        std::fs::create_dir(&root).unwrap();
        crate::commands::init::run(&root).unwrap();
        let mut repo = Repo::open_and_migrate(&root).unwrap();
        if let Some(k) = cfg.keyframe_interval {
            repo.set_keyframe_interval(k);
        }
        repos.push(repo);
    }
    let b = repos.pop().unwrap();
    let a = repos.pop().unwrap();
    let url = b.root().to_string_lossy().into_owned();
    remote::add(&a.write().unwrap(), "origin", &url).unwrap();
    Side { a, b, _dirs: dirs }
}

const FILES: [&str; 6] = [
    "f0.txt",
    "d/f1.txt",
    "d/f2.txt",
    "f3.txt",
    "f4.txt",
    "d/e/f5.txt",
];
const BRANCHES: [&str; 4] = ["main", "b0", "b1", "b2"];
const TAGS: [&str; 3] = ["t0", "t1", "t2"];
const MERGE_SOURCES: [&str; 5] = ["main", "b0", "b1", "b2", "origin/r"];

/// A file body. Five lines of which one carries the variant, so two edits of
/// different variants sometimes merge cleanly and sometimes conflict.
fn body(file: u8, variant: u8) -> Vec<u8> {
    let hot = usize::from(variant) % 5;
    (0..5)
        .map(|j| {
            let v = if j == hot { variant } else { 0 };
            format!("file {file} line {j} v{v}\n")
        })
        .collect::<String>()
        .into_bytes()
}

/// An operation. Targets are indices, resolved against the state at run time,
/// so an op list is valid on any state and replays exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Edit { file: u8, variant: Option<u8> },
    Save,
    SaveChanges { file: u8, variant: Option<u8> },
    Amend,
    Branch { n: u8 },
    Switch { n: u8, force: bool },
    Merge { n: u8 },
    Rebase { n: u8 },
    CherryPick { n: u8 },
    Undo,
    Redo,
    Squash { count: u8 },
    StashPush,
    StashPop,
    Tag { n: u8 },
    Retention,
    Compact { n: u8 },
    Gc,
    Fetch,
    Push,
    RemoteCommit { variant: u8 },
    Abort,
}

fn gen_ops(seed: u64, count: usize, mix: Mix) -> Vec<Op> {
    let mut rng = Rng::new(seed);
    let mut ops = Vec::with_capacity(count);
    for _ in 0..count {
        let n = rng.below(8) as u8;
        let file = rng.below(FILES.len()) as u8;
        let variant = if rng.below(6) == 0 {
            None
        } else {
            Some(rng.range(1, 9) as u8)
        };
        // Edits and saves dominate, so the history has something to act on.
        let op = match rng.below(40) {
            0..=7 => Op::Edit { file, variant },
            8..=13 => Op::Save,
            14 => Op::SaveChanges { file, variant },
            15 => Op::Amend,
            16 | 17 => Op::Branch { n },
            18..=20 => Op::Switch {
                n,
                force: rng.below(2) == 0,
            },
            21 | 22 => Op::Merge { n },
            23 => Op::Rebase { n },
            24 => Op::CherryPick { n },
            25 => Op::Undo,
            26 => Op::Redo,
            27 => Op::StashPush,
            28 => Op::StashPop,
            29 | 30 => Op::Tag { n },
            31 if mix.network => Op::Fetch,
            32 if mix.network => Op::Push,
            33 if mix.network => Op::RemoteCommit {
                variant: rng.range(1, 9) as u8,
            },
            34 => Op::Abort,
            35 if mix.removals => Op::Squash {
                count: rng.range(2, 3) as u8,
            },
            36 if mix.removals => Op::Compact { n },
            37 if mix.removals => Op::Retention,
            38 if mix.removals => Op::Gc,
            _ => Op::Save,
        };
        ops.push(op);
    }
    ops
}

fn branch_name(s: &str) -> BranchName {
    s.parse().unwrap()
}

/// The checked-out branch, or `main` when none is recorded yet.
fn current_branch(repo: &Repo) -> BranchName {
    branches::list(repo)
        .ok()
        .and_then(|l| l.into_iter().find(|b| b.is_current))
        .map_or_else(|| branch_name("main"), |b| b.name)
}

/// The `n`th snapshot of the whole history, in history order.
fn pick(repo: &Repo, n: u8) -> Option<SnapshotId> {
    let h = history::run(
        repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .ok()?;
    if h.entries.is_empty() {
        return None;
    }
    Some(h.entries[usize::from(n) % h.entries.len()].hash.clone())
}

fn write_file(root: &Path, file: u8, variant: Option<u8>) {
    let path = root.join(FILES[usize::from(file)]);
    match variant {
        Some(v) => {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body(file, v)).unwrap();
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Run one op on one side. `Ok` and `Err` are both results; the caller compares
/// their kinds across sides.
fn apply(side: &Side, op: &Op, step: usize) -> Result<(), Error> {
    let repo = &side.a;
    let msg = format!("step {step}");
    let spawn = Spawn::new("velo");
    match op {
        Op::Edit { file, variant } => {
            write_file(repo.root(), *file, *variant);
            Ok(())
        }
        Op::Save => save::run(&repo.write()?, Some(&msg), Default::default()).map(drop),
        Op::Amend => save::run(
            &repo.write()?,
            None,
            save::Options {
                amend: true,
                ..Default::default()
            },
        )
        .map(drop),
        Op::SaveChanges { file, variant } => {
            let g = repo.write()?;
            let branch = current_branch(repo);
            let parent = repo.branch_tip(&branch)?;
            let mut spec = SaveChanges::new(&branch, parent.as_ref(), &msg);
            spec = match variant {
                Some(v) => spec.put(TreeEntry::file(FILES[usize::from(*file)], body(*file, *v))),
                None => spec.delete(FILES[usize::from(*file)]),
            };
            g.save_changes(spec).map(drop)
        }
        Op::Branch { n } => {
            let name = branch_name(BRANCHES[usize::from(*n) % BRANCHES.len()]);
            branches::create(&repo.write()?, &name, None)
        }
        Op::Switch { n, force } => switch::run(
            &repo.write()?,
            BRANCHES[usize::from(*n) % BRANCHES.len()],
            *force,
        )
        .map(drop),
        Op::Merge { n } => merge::run(
            &repo.write()?,
            merge::Mode::Bring {
                source: MERGE_SOURCES[usize::from(*n) % MERGE_SOURCES.len()],
            },
        )
        .map(drop),
        Op::Rebase { n } => match pick(repo, *n) {
            Some(onto) => {
                rebase::run(&repo.write()?, rebase::Mode::Start { onto: &onto }, None).map(drop)
            }
            None => Ok(()),
        },
        Op::CherryPick { n } => match pick(repo, *n) {
            Some(t) => cherry_pick::run(&repo.write()?, &t, None).map(drop),
            None => Ok(()),
        },
        Op::Undo => undo::run(&repo.write()?).map(drop),
        Op::Redo => redo::run(&repo.write()?).map(drop),
        Op::Squash { count } => squash::run(&repo.write()?, usize::from(*count), &msg).map(drop),
        Op::StashPush => stash::push(&repo.write()?, None).map(drop),
        Op::StashPop => stash::pop(&repo.write()?, None).map(drop),
        Op::Tag { n } => {
            let name: TagName = TAGS[usize::from(*n) % TAGS.len()].parse().unwrap();
            tag::create(&repo.write()?, &name, None, n % 2 == 0).map(drop)
        }
        Op::Retention => {
            let policy = retention::Policy {
                tiers: vec![retention::Tier {
                    older_than_ms: 3_000,
                    keep_one_per_ms: 6_000,
                }],
            };
            let branch = current_branch(repo);
            retention::apply(&repo.write()?, &branch, &policy, Default::default()).map(drop)
        }
        Op::Compact { n } => {
            let branch = current_branch(repo);
            let h = history::run(
                repo,
                history::Options {
                    branch: Some(&branch),
                    ..Default::default()
                },
            )?;
            // Walk the first-parent chain from the tip: newest first.
            let by_hash: std::collections::HashMap<_, _> =
                h.entries.iter().map(|e| (e.hash.clone(), e)).collect();
            let mut chain = Vec::new();
            let mut at = h.entries.first().map(|e| e.hash.clone());
            while let Some(id) = at {
                if chain.len() == 4 {
                    break;
                }
                at = by_hash.get(&id).and_then(|e| e.parent.clone());
                chain.push(id);
            }
            if chain.len() < 2 {
                return Ok(());
            }
            let oldest = &chain[1 + usize::from(*n) % (chain.len() - 1)];
            let range = compact::Range {
                oldest,
                newest: &chain[0],
            };
            compact::run(&repo.write()?, &branch, &[range], Default::default()).map(drop)
        }
        Op::Gc => gc::run(
            &repo.write()?,
            gc::Options {
                keep_days: 0,
                ..Default::default()
            },
        )
        .map(drop),
        Op::Fetch => sync::fetch(&repo.write()?, "origin", &spawn, Default::default()).map(drop),
        Op::Push => {
            sync::push(&repo.write()?, "origin", None, &spawn, Default::default()).map(drop)
        }
        Op::RemoteCommit { variant } => {
            let g = side.b.write()?;
            let branch = branch_name("r");
            let parent = side.b.branch_tip(&branch)?;
            g.save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: None,
                message: &msg,
                entries: vec![TreeEntry::file("remote.txt", body(9, *variant))],
                meta: Default::default(),
                author: None,
                timestamp_ms: None,
                renames: &[],
            })
            .map(drop)
        }
        Op::Abort => {
            let g = repo.write()?;
            let m = merge::run(&g, merge::Mode::Abort).map(drop);
            let r = rebase::run(&g, rebase::Mode::Abort, None).map(drop);
            m.and(r)
        }
    }
}

/// Render a result so that equal results compare equal across sides: the debug
/// form on success, the stable variant name on failure.
fn show<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    match r {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("error:{}", e.variant_name()),
    }
}

/// Everything that must match between the two sides, as named sections.
fn digest(repo: &Repo) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();

    let ids = |table: &str| -> Vec<String> {
        let mut stmt = repo
            .conn()
            .prepare(&format!("SELECT hash FROM {table} ORDER BY hash"))
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    let live = ids("snapshots");
    out.push(("snapshot ids", format!("{live:?}")));
    out.push(("trash ids", format!("{:?}", ids("trash"))));

    let mut trees = String::new();
    for id in &live {
        let tree = repo.tree_at(&SnapshotId::from_stored(id.clone()));
        let rendered = tree.map(|t| {
            t.into_iter()
                .map(|f| format!("{}|{:?}|{:?}", f.path, f.object, f.kind))
                .collect::<Vec<_>>()
        });
        trees.push_str(&format!("{id}: {}\n", show(rendered)));
    }
    out.push(("trees", trees));

    let list = branches::list(repo);
    let mut hist = String::new();
    let mut blames = String::new();
    if let Ok(list) = &list {
        for b in list {
            let h = history::run(
                repo,
                history::Options {
                    branch: Some(&b.name),
                    ..Default::default()
                },
            );
            hist.push_str(&format!("{}: {}\n", b.name, show(h.map(|h| h.entries))));
            let Some(tip) = &b.tip else { continue };
            let Ok(tree) = repo.tree_at(&tip.hash) else {
                continue;
            };
            for f in tree.iter().take(3) {
                let r = blame::run(
                    repo,
                    Path::new(&f.path),
                    blame::Options {
                        at: Some(&tip.hash),
                        ..Default::default()
                    },
                );
                blames.push_str(&format!(
                    "{}:{}: {}\n",
                    b.name,
                    f.path,
                    show(r.map(|b| b.lines))
                ));
            }
        }
    }
    out.push(("branches", show(list)));
    out.push(("history", hist));
    out.push(("blame", blames));

    let st = status::run(repo, &[]).map(|mut s| {
        s.new_files.sort();
        s.modified.sort();
        s.deleted.sort();
        s.conflicts.sort();
        s
    });
    out.push(("status", show(st)));
    out
}

fn assert_green(repo: &Repo, which: &str) -> Result<(), String> {
    match fsck::check(repo) {
        Ok(r) if r.is_healthy() => Ok(()),
        Ok(r) => Err(format!("fsck is red on {which}: {:?}", r.problems)),
        Err(e) => Err(format!("fsck failed to run on {which}: {e}")),
    }
}

fn compare(reference: &Side, subject: &Side) -> Result<(), String> {
    for (label, r, s) in [
        ("local", &reference.a, &subject.a),
        ("remote", &reference.b, &subject.b),
    ] {
        let (dr, ds) = (digest(r), digest(s));
        for ((name, x), (_, y)) in dr.iter().zip(&ds) {
            if x != y {
                return Err(format!(
                    "{label} {name} differ\n--- reference\n{x}\n--- subject\n{y}"
                ));
            }
        }
        assert_green(r, &format!("reference {label}"))?;
        assert_green(s, &format!("subject {label}"))?;
    }
    Ok(())
}

/// A failed run: where, and why.
#[derive(Debug)]
struct Failure {
    step: usize,
    why: String,
}

/// Drive `ops` through a fresh reference and a fresh subject, comparing after
/// every one.
fn run_ops(reference: &Config, subject: &Config, ops: &[Op]) -> Result<(), Failure> {
    const BASE_MS: i64 = 1_700_000_000_000;
    let (r, s) = (side(reference), side(subject));
    for (step, op) in ops.iter().enumerate() {
        let now = BASE_MS + step as i64 * 1_000;
        for repo in [&r.a, &r.b, &s.a, &s.b] {
            repo.test_now_ms.set(Some(now));
        }
        let fail = |why: String| Failure {
            step,
            why: format!("{op:?}: {why}"),
        };
        let (x, y) = (show(apply(&r, op, step)), show(apply(&s, op, step)));
        // Only the kind of an error has to match; a success must match fully.
        let kind = |v: &str| v.starts_with("error:").then(|| v.to_string());
        if kind(&x) != kind(&y) || (kind(&x).is_none() && x != y) {
            return Err(fail(format!("outcome differs: {x} vs {y}")));
        }
        compare(&r, &s).map_err(fail)?;
    }
    Ok(())
}

/// Find the shortest op list that still fails: the shortest failing prefix
/// first, then greedily drop single ops that are not needed.
fn shrink(reference: &Config, subject: &Config, ops: &[Op]) -> Vec<Op> {
    let mut best: Vec<Op> = ops.to_vec();
    for len in 1..=ops.len() {
        if run_ops(reference, subject, &ops[..len]).is_err() {
            best = ops[..len].to_vec();
            break;
        }
    }
    let mut i = 0;
    while i < best.len() {
        let mut trial = best.clone();
        trial.remove(i);
        if !trial.is_empty() && run_ops(reference, subject, &trial).is_err() {
            best = trial;
        } else {
            i += 1;
        }
    }
    best
}

fn check_seed(reference: &Config, subject: &Config, seed: u64, count: usize, mix: Mix) {
    let ops = gen_ops(seed, count, mix);
    if let Err(f) = run_ops(reference, subject, &ops) {
        let minimal = shrink(reference, subject, &ops);
        panic!(
            "differential failure, seed {seed} ({count} ops), at step {}: {}\n\
             replay with VELO_DIFF_SEED={seed} VELO_DIFF_OPS={count}\n\
             shortest failing ops:\n{minimal:#?}",
            f.step, f.why
        );
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn op_count() -> usize {
    env_u64("VELO_DIFF_OPS").map_or(40, |n| n as usize)
}

#[test]
fn differential_smoke() {
    // Five runs keep the debug build near 20 s; the mix alternates so both the
    // removing and the append-only operation sets are covered.
    // The reference stores every tree whole; the subject runs at several K so
    // short chains, keyframes mid-history and the default are all compared with
    // it. Removals and the network stay off until 20.6-b and 20.4-b.
    let seeds: Vec<u64> = match env_u64("VELO_DIFF_SEED") {
        Some(s) => vec![s],
        None => vec![1, 2, 3],
    };
    let mix = Mix {
        removals: false,
        network: false,
    };
    let reference = Config {
        keyframe_interval: Some(1),
    };
    for k in [2, 3, 128] {
        let subject = Config {
            keyframe_interval: Some(k),
        };
        for &seed in &seeds {
            check_seed(&reference, &subject, seed, op_count(), mix);
        }
    }
}

#[test]
fn differential_replays_a_seed() {
    let mix = Mix {
        removals: true,
        network: true,
    };
    assert_eq!(gen_ops(77, 40, mix), gen_ops(77, 40, mix));
    assert_ne!(gen_ops(77, 40, mix), gen_ops(78, 40, mix));
    assert!(gen_ops(
        77,
        200,
        Mix {
            removals: false,
            network: false,
        },
    )
    .iter()
    .all(|op| !matches!(
        op,
        Op::Squash { .. } | Op::Compact { .. } | Op::Retention | Op::Gc
    )));
}

/// Runs until `VELO_DIFF_SECS` (default 60) expires, a fresh seed per
/// iteration, printed so a failure can be replayed.
#[test]
#[allow(clippy::print_stderr)]
#[ignore = "long run: set VELO_DIFF_SECS, optionally VELO_DIFF_SEED as the first seed"]
fn differential_long() {
    let secs = env_u64("VELO_DIFF_SECS").unwrap_or(60);
    let first = env_u64("VELO_DIFF_SEED").unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_secs())
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let reference = Config {
        keyframe_interval: Some(1),
    };
    let subject = Config {
        keyframe_interval: Some(3),
    };
    let mut i = 0;
    while Instant::now() < deadline {
        let seed = first.wrapping_add(i);
        eprintln!("differential_long: seed {seed}");
        check_seed(
            &reference,
            &subject,
            seed,
            op_count(),
            Mix {
                removals: false,
                network: false,
            },
        );
        i += 1;
    }
}
