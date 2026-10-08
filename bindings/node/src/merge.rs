//! Merge planning and recording, and branch management.
//!
//! Merging here never touches a working tree: `mergePlan` reads, `mergeCommit`
//! records a snapshot, and conflicts are settled by the caller's resolutions.

use std::collections::BTreeMap;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use velo_core::commands::branches;
use velo_core::commands::merge::{self, FileAction, MergeCommit, PlannedChange, Resolution};

use crate::repo::{
    branch_name, build_author, build_meta, snapshot_id, AuthorInput, Job, Meta, Repo,
};

/// One path a merge would touch.
#[napi(object)]
pub struct PlannedFileInfo {
    pub path: String,
    /// `'deleted'`, `'added'`, `'updated'`, `'autoMerged'`, `'keptOurs'` or `'conflicted'`.
    pub action: String,
    /// The object to take, for `'added'` and `'updated'`.
    pub object: Option<String>,
    pub mode: Option<i64>,
    /// The merged bytes, for `'autoMerged'`.
    pub content: Option<Buffer>,
    /// For `'conflicted'`: each side's object, absent where that side lacks the file.
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

#[napi(object)]
pub struct MergePlanInfo {
    /// The common ancestor, or `null` when the two have no shared history.
    #[napi(ts_type = "string | null")]
    pub base: Either<String, Null>,
    pub files: Vec<PlannedFileInfo>,
}

/// A merge to record.
#[napi(object)]
pub struct MergeCommitInput {
    pub branch: String,
    pub ours: String,
    pub theirs: String,
    pub message: String,
    /// Per conflicted path: `'ours'`, `'theirs'`, `null` to delete it, or the
    /// bytes to write. A string other than `'ours'`/`'theirs'` is written as
    /// UTF-8 content; pass a `Buffer` to write bytes that may spell either word.
    #[napi(ts_type = "Record<string, 'ours' | 'theirs' | null | Buffer | string>")]
    pub resolutions: Option<BTreeMap<String, Either3<Buffer, String, Null>>>,
    #[napi(ts_type = "Record<string, Record<string, string>>")]
    pub meta: Option<Meta>,
    pub author: Option<AuthorInput>,
    pub timestamp_ms: Option<i64>,
}

/// A branch and where it points.
#[napi(object)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    /// The tip snapshot, or `null` for a branch with no snapshots yet.
    #[napi(ts_type = "string | null")]
    pub tip: Either<String, Null>,
}

fn action_name(action: FileAction) -> &'static str {
    match action {
        FileAction::Deleted => "deleted",
        FileAction::Added => "added",
        FileAction::Updated => "updated",
        FileAction::AutoMerged => "autoMerged",
        FileAction::KeptOurs => "keptOurs",
        FileAction::Conflicted => "conflicted",
    }
}

fn plan_info(plan: merge::MergePlan) -> MergePlanInfo {
    let id = |h: &velo_core::ObjectHash| h.as_str().to_string();
    let files = plan
        .files
        .into_iter()
        .map(|f| {
            let mut info = PlannedFileInfo {
                path: f.path,
                action: action_name(f.change.action()).to_string(),
                object: None,
                mode: None,
                content: None,
                base: None,
                ours: None,
                theirs: None,
            };
            match f.change {
                PlannedChange::Take { object, mode, .. } => {
                    info.object = Some(id(&object));
                    info.mode = Some(mode);
                }
                PlannedChange::AutoMerge { content, mode } => {
                    info.content = Some(Buffer::from(content));
                    info.mode = Some(mode);
                }
                PlannedChange::Conflict { base, ours, theirs } => {
                    info.base = base.as_ref().map(id);
                    info.ours = ours.as_ref().map(id);
                    info.theirs = theirs.as_ref().map(id);
                }
                _ => {}
            }
            info
        })
        .collect();
    MergePlanInfo {
        base: match plan.base {
            Some(b) => Either::A(b.as_str().to_string()),
            None => Either::B(Null),
        },
        files,
    }
}

/// A resolution with the `Buffer` copied out, so it can cross to the worker.
enum RawResolution {
    Delete,
    Bytes(Vec<u8>),
    Text(String),
}

#[napi]
impl Repo {
    /// The nearest snapshot that is an ancestor of both, or `null`.
    #[napi(ts_return_type = "Promise<string | null>")]
    pub fn merge_base(&self, a: String, b: String) -> AsyncTask<Job<Option<String>>> {
        self.job(move |repo| {
            let base = merge::merge_base(repo, &snapshot_id(&a)?, &snapshot_id(&b)?)?;
            Ok(base.map(|s| s.as_str().to_string()))
        })
    }

    /// What merging `theirs` into `ours` would do, without doing it.
    #[napi(ts_return_type = "Promise<MergePlanInfo>")]
    pub fn merge_plan(&self, ours: String, theirs: String) -> AsyncTask<Job<MergePlanInfo>> {
        self.job(move |repo| {
            let plan = merge::plan(repo, &snapshot_id(&ours)?, &snapshot_id(&theirs)?)?;
            Ok(plan_info(plan))
        })
    }

    /// Record the merge as a snapshot and resolve to its id. Rejects with code
    /// `'Conflicts'` and `paths` when a conflict has no resolution.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn merge_commit(&self, input: MergeCommitInput) -> AsyncTask<Job<String>> {
        let MergeCommitInput {
            branch,
            ours,
            theirs,
            message,
            resolutions,
            meta,
            author,
            timestamp_ms,
        } = input;
        let raw: Vec<(String, RawResolution)> = resolutions
            .unwrap_or_default()
            .into_iter()
            .map(|(path, r)| {
                let r = match r {
                    Either3::A(buf) => RawResolution::Bytes(buf.to_vec()),
                    Either3::B(text) => RawResolution::Text(text),
                    Either3::C(_) => RawResolution::Delete,
                };
                (path, r)
            })
            .collect();
        self.job(move |repo| {
            let branch = branch_name(&branch)?;
            let ours = snapshot_id(&ours)?;
            let theirs = snapshot_id(&theirs)?;
            let resolutions: Vec<(String, Resolution)> = raw
                .into_iter()
                .map(|(path, r)| {
                    let r = match r {
                        RawResolution::Delete => Resolution::Delete,
                        RawResolution::Bytes(bytes) => Resolution::Content(bytes),
                        RawResolution::Text(text) => match text.as_str() {
                            "ours" => Resolution::Ours,
                            "theirs" => Resolution::Theirs,
                            _ => Resolution::Content(text.into_bytes()),
                        },
                    };
                    (path, r)
                })
                .collect();
            let meta = build_meta(meta)?;
            let author = build_author(author)?;
            let guard = repo.write()?;
            let id = merge::commit(
                &guard,
                MergeCommit {
                    branch: &branch,
                    ours: &ours,
                    theirs: &theirs,
                    resolutions: &resolutions,
                    message: &message,
                    meta,
                    author: author.as_ref(),
                    timestamp_ms,
                },
            )?;
            drop(guard);
            Ok(id.as_str().to_string())
        })
    }

    /// Every branch, sorted by name.
    #[napi(ts_return_type = "Promise<Array<BranchInfo>>")]
    pub fn branches(&self) -> AsyncTask<Job<Vec<BranchInfo>>> {
        self.job(|repo| {
            Ok(branches::list(repo)?
                .into_iter()
                .map(|b| BranchInfo {
                    name: b.name.as_str().to_string(),
                    is_current: b.is_current,
                    tip: match b.tip {
                        Some(t) => Either::A(t.hash.as_str().to_string()),
                        None => Either::B(Null),
                    },
                })
                .collect())
        })
    }

    /// Create a branch, at snapshot `at` or unborn when absent.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn create_branch(&self, name: String, at: Option<String>) -> AsyncTask<Job<()>> {
        self.job(move |repo| {
            let name = branch_name(&name)?;
            let at = at.as_deref().map(snapshot_id).transpose()?;
            let guard = repo.write()?;
            branches::create(&guard, &name, at.as_ref())
        })
    }

    /// Point an existing branch at snapshot `to`.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn set_branch_tip(&self, name: String, to: String) -> AsyncTask<Job<()>> {
        self.job(move |repo| {
            let name = branch_name(&name)?;
            let to = snapshot_id(&to)?;
            let guard = repo.write()?;
            branches::set_tip(&guard, &name, &to)
        })
    }
}
