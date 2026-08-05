//! Rendering: turning `velo-core`'s data into terminal output.
//!
//! All colour, layout and phrasing lives here. Core returns structs; this module
//! is the only place that decides how they look, which is what lets another
//! consumer present the same data completely differently.

pub mod apply;
pub mod blame;
pub mod branches;
pub mod bundle;
pub mod cherry_pick;
pub mod diff;
pub mod fsck;
pub mod gc;
pub mod grep;
pub mod history;
pub mod id;
pub mod init;
pub mod merge;
pub mod progress;
pub mod rebase;
pub mod remote;
pub mod restore;
pub mod save;
pub mod squash;
pub mod stash;
pub mod status;
pub mod switch;
pub mod sync;
pub mod tag;
pub mod undo;
pub mod when;
