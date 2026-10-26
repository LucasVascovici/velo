//! Rename-aware merging, built on the recorded `renames` edges.
//!
//! A merge works path by path, so a file one side moved and the other side
//! edited looks like "deleted here, modified there". Rather than detect
//! renames by content similarity, this module reads only the edges that
//! `velody mv` and friends recorded, and remaps the other side's trees so the
//! ordinary three-way merge sees the edit at the new path.

use std::collections::HashSet;

use crate::commands::apply;
use crate::error::Result;

/// Which side of a merge moved a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MovedBy {
    Ours,
    Theirs,
}

/// A file one side moved that the other side still knew by its old name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Moved {
    pub from: String,
    pub to: String,
    pub by: MovedBy,
}

/// The renames recorded on snapshots reachable from `tip` but not from `base`,
/// as final `(from, to)` pairs.
///
/// Chains are composed in creation order, so F->G followed by G->H comes back
/// as F->H: the merge base only knows the file as F.
pub(crate) fn recorded_moves(
    conn: &rusqlite::Connection,
    base: Option<&str>,
    tip: &str,
) -> Result<Vec<(String, String)>> {
    let tip_side = crate::commands::ancestors(conn, tip)?;
    let base_side = match base {
        Some(b) => crate::commands::ancestors(conn, b)?,
        None => Default::default(),
    };
    let mut stmt = conn.prepare(
        "SELECT r.snapshot_hash, r.from_path, r.to_path
           FROM renames r JOIN snapshots s ON s.hash = r.snapshot_hash
          ORDER BY s.created_at_ms, r.rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut moves: Vec<(String, String)> = Vec::new();
    for row in rows {
        let (snapshot, from, to) = row?;
        if !tip_side.contains_key(&snapshot) || base_side.contains_key(&snapshot) {
            continue;
        }
        match moves.iter_mut().find(|(_, t)| *t == from) {
            Some(existing) => existing.1 = to,
            None => moves.push((from, to)),
        }
    }
    Ok(moves)
}

/// Remap the trees so a file one side moved is merged at its new path.
///
/// For each of our recorded moves where the base has `from`, we lack it and
/// have `to`, and they still have `from` and lack `to`, the base's and their
/// entries move from `from` to `to`. Our tree is untouched, so it stays
/// consistent with the working tree. The old path then exists in no tree and is
/// never recreated.
///
/// The mirror case is a move recorded on their side while we still have the
/// file at `from`: the base's and our entries move to `to`. Our working tree
/// still holds the file at `from`, so the caller has to move it on disk (see
/// `apply::reconcile_tree_moving`); the returned `Moved` says so.
#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_moves(
    conn: &rusqlite::Connection,
    base: Option<&str>,
    ours: &str,
    theirs: &str,
    base_tree: &mut apply::Tree,
    our_tree: &mut apply::Tree,
    their_tree: &mut apply::Tree,
) -> Result<Vec<Moved>> {
    let mut done = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    for (from, to) in recorded_moves(conn, base, ours)? {
        if base_tree.contains_key(&from)
            && !base_tree.contains_key(&to)
            && !our_tree.contains_key(&from)
            && our_tree.contains_key(&to)
            && their_tree.contains_key(&from)
            && !their_tree.contains_key(&to)
            && taken.insert(to.clone())
        {
            let b = base_tree.remove(&from).expect("checked above");
            let t = their_tree.remove(&from).expect("checked above");
            base_tree.insert(to.clone(), b);
            their_tree.insert(to.clone(), t);
            done.push(Moved {
                from,
                to,
                by: MovedBy::Ours,
            });
        }
    }
    for (from, to) in recorded_moves(conn, base, theirs)? {
        if base_tree.contains_key(&from)
            && !base_tree.contains_key(&to)
            && !their_tree.contains_key(&from)
            && their_tree.contains_key(&to)
            && our_tree.contains_key(&from)
            && !our_tree.contains_key(&to)
            && taken.insert(to.clone())
        {
            let b = base_tree.remove(&from).expect("checked above");
            let o = our_tree.remove(&from).expect("checked above");
            base_tree.insert(to.clone(), b);
            our_tree.insert(to.clone(), o);
            done.push(Moved {
                from,
                to,
                by: MovedBy::Theirs,
            });
        }
    }
    Ok(done)
}
