//! Shared builders for the profiling harness in `perf_profile.rs`: a small
//! deterministic generator, the two project layouts the benchmarks use, the
//! agent's edit pattern, and two ways to grow a long history. Phase 19.1-b.
//!
//! Nothing here is timed. These helpers exist so the measurement tests spend
//! their effort on the operation being measured, not on building its input.

use std::collections::HashMap;

use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta};

const WORDS: [&str; 24] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi",
    "psi", "omega",
];

const LINES_PER_FILE: usize = 20;

/// xorshift64*: deterministic and dependency-free. The edits it drives match the
/// benchmark scripts' distribution, not their exact bytes.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        // A zero state would stay zero forever.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform-enough value in `0..n`. `n` must be non-zero.
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// A value in `lo..=hi`.
    pub(super) fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len())]
    }
}

/// Which project shape to generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    /// `src/d{i % 20:02}/f{i:04}.txt`, 20 lines of about 60 bytes: the agent
    /// benchmark's `make_project`.
    Agent,
    /// `d{d/10:03}/s{d%10}/f{i:06}.txt` with `d = i / 100` and about 200-byte
    /// files: the scale benchmark's `rel()`.
    Scale,
}

fn agent_line(rng: &mut Rng, n: usize) -> String {
    let mut line = format!("L{n}");
    for _ in 0..8 {
        line.push(' ');
        line.push_str(rng.word());
    }
    line.push_str(&format!(" {}", rng.below(1_000_000)));
    line
}

/// `files` files of the given layout, as `(path, bytes)`.
pub(super) fn project(layout: Layout, files: usize, rng: &mut Rng) -> Vec<(String, Vec<u8>)> {
    (0..files)
        .map(|i| match layout {
            Layout::Agent => {
                let path = format!("src/d{:02}/f{i:04}.txt", i % 20);
                let mut text = String::new();
                for n in 0..LINES_PER_FILE {
                    text.push_str(&agent_line(rng, n));
                    text.push('\n');
                }
                (path, text.into_bytes())
            }
            Layout::Scale => {
                let d = i / 100;
                let path = format!("d{:03}/s{}/f{i:06}.txt", d / 10, d % 10);
                let mut text = format!("file {i}\n");
                while text.len() < 200 {
                    text.push_str(rng.word());
                    text.push(' ');
                }
                text.push('\n');
                (path, text.into_bytes())
            }
        })
        .collect()
}

/// One agent step: 1 to 20 files, 1 to 3 lines replaced in each, the line count
/// unchanged. Mirrors `make_edit` in `bench/agent/agent_bench.py`.
pub(super) fn agent_edit(
    rng: &mut Rng,
    paths: &[String],
    contents: &HashMap<String, Vec<u8>>,
) -> Vec<(String, Vec<u8>)> {
    let count = rng.range(1, 20).min(paths.len());
    let mut chosen: Vec<usize> = Vec::new();
    while chosen.len() < count {
        let i = rng.below(paths.len());
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    chosen
        .into_iter()
        .map(|i| {
            let path = &paths[i];
            let text = String::from_utf8_lossy(&contents[path]).into_owned();
            let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
            for _ in 0..rng.range(1, 3) {
                let n = rng.below(LINES_PER_FILE);
                let mut line = format!("L{n} edit");
                for _ in 0..5 {
                    line.push(' ');
                    line.push_str(rng.word());
                }
                lines[n] = line;
            }
            (path.clone(), lines.join("\n").into_bytes())
        })
        .collect()
}

fn created_at(repo: &Repo, id: &SnapshotId) -> i64 {
    repo.conn()
        .query_row(
            "SELECT created_at_ms FROM snapshots WHERE hash = ?",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

/// Add `snapshots` snapshots to `branch` after `base`, each changing one file,
/// and return their ids oldest first.
///
/// `bulk = false` goes through `save_tree`: every unchanged file is a
/// `TreeEntry::stored`, one file carries new bytes, and `timestamp_ms` rises by
/// one per step so the order is unambiguous. It is the honest path and costs
/// the size of the tree per snapshot.
///
/// `bulk = true` writes the rows directly: a `snapshots` row, the parent's
/// `file_map` rows copied with one `INSERT ... SELECT`, then one `UPDATE`
/// pointing a row at an object that already exists. It reaches large table
/// sizes quickly, but the ids are synthetic 64-hex values and do NOT verify:
/// `fsck` fails on them. They exist only to reach large table sizes, never to be
/// kept. Each snapshot is one transaction.
pub(super) fn build_history(
    repo: &Repo,
    branch: &BranchName,
    base: &SnapshotId,
    snapshots: usize,
    bulk: bool,
    rng: &mut Rng,
) -> Vec<SnapshotId> {
    let mut ids = Vec::with_capacity(snapshots);
    let mut parent = base.clone();
    let base_ms = created_at(repo, base);
    if bulk {
        let conn = repo.conn();
        for n in 0..snapshots {
            let id = blake3::hash(format!("bulk {parent} {n} {}", rng.next_u64()).as_bytes())
                .to_hex()
                .to_string();
            let rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM file_map WHERE snapshot_hash = ?",
                    [parent.as_str()],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(rows > 0, "the base snapshot has no files");
            let changed = rng.below(rows as usize) as i64;
            let donor = rng.below(rows as usize) as i64;
            conn.execute_batch("BEGIN").unwrap();
            conn.execute(
                "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
                 VALUES (?, ?, ?, ?, '', ?)",
                rusqlite::params![
                    id,
                    format!("bulk {n}"),
                    branch.as_str(),
                    parent.as_str(),
                    base_ms + 1 + n as i64
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO file_map SELECT ?, path, hash, mode FROM file_map WHERE snapshot_hash = ?",
                [id.as_str(), parent.as_str()],
            )
            .unwrap();
            let donor_hash: String = conn
                .query_row(
                    "SELECT hash FROM file_map WHERE snapshot_hash = ? LIMIT 1 OFFSET ?",
                    rusqlite::params![id, donor],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "UPDATE file_map SET hash = ?1 WHERE snapshot_hash = ?2 AND path =
                   (SELECT path FROM file_map WHERE snapshot_hash = ?2 LIMIT 1 OFFSET ?3)",
                rusqlite::params![donor_hash, id, changed],
            )
            .unwrap();
            conn.execute_batch("COMMIT").unwrap();
            parent = SnapshotId::from_stored(id);
            ids.push(parent.clone());
        }
        return ids;
    }

    for n in 0..snapshots {
        let mut entries: Vec<TreeEntry> = repo
            .tree_at(&parent)
            .unwrap()
            .into_iter()
            .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
            .collect();
        let at = rng.below(entries.len());
        let path = entries[at].path.clone();
        entries[at] = TreeEntry::file(
            path,
            format!("history {n} {}\n", rng.next_u64()).into_bytes(),
        );
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch,
                parent: Some(&parent),
                merge_parent: None,
                message: &format!("history {n}"),
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(base_ms + 1 + n as i64),
                renames: &[],
            })
            .unwrap();
        parent = id;
        ids.push(parent.clone());
    }
    ids
}

/// The object a file's bytes are stored under: the hash of the CRLF-normalised
/// content.
pub(super) fn predict_object(bytes: &[u8]) -> ObjectHash {
    let normal = crate::storage::normalise_crlf(bytes.to_vec());
    blake3::hash(&normal).to_hex().to_string().parse().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seeded(files: usize) -> (TempDir, Repo, SnapshotId) {
        let tmp = TempDir::new().unwrap();
        crate::commands::init::run(tmp.path()).unwrap();
        let repo = Repo::open_and_migrate(tmp.path()).unwrap();
        let mut rng = Rng::new(1);
        let entries = project(Layout::Scale, files, &mut rng)
            .into_iter()
            .map(|(p, b)| TreeEntry::file(p, b))
            .collect();
        let main: BranchName = "main".parse().unwrap();
        let base = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &main,
                parent: None,
                merge_parent: None,
                message: "base",
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(1),
                renames: &[],
            })
            .unwrap();
        (tmp, repo, base)
    }

    fn count(repo: &Repo, sql: &str) -> i64 {
        repo.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn both_history_builders_add_the_expected_rows() {
        for bulk in [false, true] {
            let (_t, repo, base) = seeded(20);
            let main: BranchName = "main".parse().unwrap();
            let ids = build_history(&repo, &main, &base, 5, bulk, &mut Rng::new(2));
            assert_eq!(ids.len(), 5);
            assert_eq!(
                count(&repo, "SELECT COUNT(*) FROM snapshots"),
                6,
                "bulk={bulk}"
            );
            assert_eq!(
                count(&repo, "SELECT COUNT(*) FROM file_map"),
                120,
                "bulk={bulk}"
            );
            assert_eq!(repo.tree_at(ids.last().unwrap()).unwrap().len(), 20);
            if bulk {
                assert!(ids.iter().all(|i| i.as_str().len() == 64));
            }
        }
    }

    #[test]
    fn agent_edit_keeps_the_line_count() {
        let mut rng = Rng::new(3);
        let files = project(Layout::Agent, 30, &mut rng);
        let paths: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
        let contents: HashMap<String, Vec<u8>> = files.into_iter().collect();
        for _ in 0..20 {
            let edits = agent_edit(&mut rng, &paths, &contents);
            assert!((1..=20).contains(&edits.len()));
            for (path, bytes) in edits {
                let lines = |b: &[u8]| b.iter().filter(|&&c| c == b'\n').count();
                assert_eq!(lines(&bytes), lines(&contents[&path]));
                assert_ne!(bytes, contents[&path]);
            }
        }
    }

    #[test]
    fn layouts_have_the_documented_paths() {
        let mut rng = Rng::new(4);
        assert_eq!(
            project(Layout::Agent, 21, &mut rng)[20].0,
            "src/d00/f0020.txt"
        );
        let scale = project(Layout::Scale, 1001, &mut rng);
        assert_eq!(scale[1000].0, "d001/s0/f001000.txt");
        assert!(scale[0].1.len() >= 200);
    }
}
