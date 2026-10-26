//! Change events: what was committed, delivered as it is committed.
//!
//! [`Repo::head_token`](crate::Repo::head_token) answers "did anything change?"
//! by polling. A GUI or a webhook bridge wants "what changed", so a handle can
//! carry [`Listener`]s (see [`Repo::listening`](crate::Repo::listening)) that
//! hear [`Event::Saved`], [`Event::Merged`], [`Event::RefMoved`] and
//! [`Event::Imported`] as they are committed.
//!
//! # Guarantees
//!
//! - Events are emitted **after** the transaction commits, never before. An
//!   operation that fails or rolls back emits nothing.
//! - Listeners are called synchronously, on the writing thread, in registration
//!   order. A slow listener slows the writer; hand off to a channel if that
//!   matters — `mpsc::Sender<Event>` is a listener already.
//! - Delivery is **per handle**. A listener on one [`Repo`](crate::Repo) does not
//!   hear writes made through another `Repo`, even in the same process. That is
//!   by design: there is no global registry to hold them. Changes made by other
//!   handles or other processes are seen by polling `head_token`, which is honest
//!   about what SQLite can tell us.
//! - Stash snapshots (on the internal `_stash` branch) emit nothing: they are
//!   bookkeeping, not history a user would want announced.
//! - Working-tree-only changes (restore, switch) emit nothing: nothing was
//!   committed.

use crate::{BranchName, SnapshotId, TagName};

/// Something that was committed to the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A new snapshot row was committed.
    Saved {
        /// The new snapshot.
        snapshot: SnapshotId,
        /// The branch it was saved on.
        branch: BranchName,
        /// Its first parent, if any.
        parent: Option<SnapshotId>,
        /// Its second parent, if it joins two lines of history.
        merge_parent: Option<SnapshotId>,
    },
    /// A snapshot joining two lines of history was committed. Emitted right
    /// after its [`Event::Saved`].
    Merged {
        /// The merge snapshot.
        snapshot: SnapshotId,
        /// The branch it landed on.
        into: BranchName,
        /// The first parent.
        ours: SnapshotId,
        /// The second parent.
        theirs: SnapshotId,
    },
    /// An explicit ref was created, moved or removed. `None` means absent or
    /// unborn.
    RefMoved {
        /// Which ref moved.
        reference: Ref,
        /// Where it pointed before.
        from: Option<SnapshotId>,
        /// Where it points now.
        to: Option<SnapshotId>,
    },
    /// History arrived from elsewhere: a bundle apply, fetch, pull, clone, or a
    /// push received locally.
    Imported {
        /// How many snapshots were new to this repository.
        snapshots: usize,
    },
}

/// A named pointer into history.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ref {
    /// A branch.
    Branch(BranchName),
    /// A tag.
    Tag(TagName),
}

/// Receives events, synchronously on the writing thread, after the transaction
/// commits.
pub trait Listener: Send + Sync {
    /// Called once per event.
    fn notify(&self, event: &Event);
}

impl Listener for std::sync::mpsc::Sender<Event> {
    fn notify(&self, event: &Event) {
        // A dropped receiver is the subscriber going away, not a failure of the
        // write that already committed.
        let _ = self.send(event.clone());
    }
}
