//! History, metadata search and blame.
//!
//! These resolve to objects that hold a `Date`, which can only be made on the
//! JS thread, so each has a `ScopedTask`: the worker collects plain data and
//! `resolve` turns it into JS values.

use std::path::{Path, PathBuf};

use napi::bindgen_prelude::*;
use napi::{Env, JsDate, ScopedTask};
use napi_derive::napi;
use velo_core::commands::{blame, history};

use crate::errors::to_js;
use crate::repo::{
    branch_name, nullable, snapshot_id, CoreResult, Nullable, Repo, Shared, SnapshotData,
    SnapshotInfo,
};

/// One metadata condition. Without `value`, it matches any value of the key.
#[napi(object)]
pub struct MetaFilterInput {
    pub namespace: String,
    pub key: String,
    pub value: Option<String>,
}

fn filters(meta: &[MetaFilterInput]) -> Vec<history::MetaFilter<'_>> {
    meta.iter()
        .map(|m| match &m.value {
            Some(v) => history::MetaFilter::equals(&m.namespace, &m.key, v),
            None => history::MetaFilter::has(&m.namespace, &m.key),
        })
        .collect()
}

/// What `history` filters on. Every field is optional.
#[napi(object)]
pub struct HistoryOptions {
    /// List the ancestry of this snapshot, following both parents of a merge.
    /// Without `from` or `branch`, velo walks back from the working tree's
    /// position, so a repository with no working tree must pass one of them.
    pub from: Option<String>,
    /// Only snapshots recorded on this branch.
    pub branch: Option<String>,
    /// Every branch. Ignored when `branch` is set.
    pub all: Option<bool>,
    /// Only snapshots that changed one of these paths (or anything under one).
    pub paths: Option<Vec<String>>,
    /// The newest N matches.
    pub limit: Option<u32>,
    /// Only snapshots satisfying every one of these.
    pub meta: Option<Vec<MetaFilterInput>>,
}

type EntriesWork = Box<dyn FnOnce(&velo_core::Repo) -> CoreResult<Vec<SnapshotData>> + Send>;

pub struct EntriesJob {
    repo: Shared,
    run: Option<EntriesWork>,
}

impl<'task> ScopedTask<'task> for EntriesJob {
    type Output = CoreResult<Vec<SnapshotData>>;
    type JsValue = Vec<SnapshotInfo<'task>>;

    fn compute(&mut self) -> Result<Self::Output> {
        let run = self
            .run
            .take()
            .ok_or_else(|| napi::Error::from_reason("velo job already ran"))?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| napi::Error::from_reason("velo Repo is poisoned by an earlier panic"))?;
        Ok(run(&repo))
    }

    fn resolve(&mut self, env: &'task Env, output: Self::Output) -> Result<Self::JsValue> {
        output
            .map_err(|e| to_js(env, e))?
            .into_iter()
            .map(|d| d.into_info(env))
            .collect()
    }
}

/// The author of a snapshot.
#[napi(object)]
pub struct AuthorInfo {
    pub name: String,
    #[napi(ts_type = "string | null")]
    pub email: Nullable,
}

/// The snapshot a line came from.
#[napi(object)]
pub struct LineOriginInfo<'env> {
    pub id: String,
    pub created_at_ms: i64,
    pub created_at: JsDate<'env>,
    pub message: String,
    #[napi(ts_type = "AuthorInfo | null")]
    pub author: Either<AuthorInfo, Null>,
    pub branch: String,
    /// The file's path at that snapshot, which differs after a rename.
    pub path: String,
}

#[napi(object)]
pub struct BlameLineInfo<'env> {
    /// 1-based line number as of the blamed snapshot.
    pub line_no: u32,
    pub text: String,
    /// How many file lines this unit spans; 1 unless a driver groups lines.
    pub line_count: u32,
    /// `null` when history does not explain the line.
    #[napi(ts_type = "LineOriginInfo | null")]
    pub origin: Either<LineOriginInfo<'env>, Null>,
}

#[napi(object)]
pub struct BlameInfo<'env> {
    pub path: String,
    /// The snapshot the file was read at.
    pub snapshot: String,
    pub lines: Vec<BlameLineInfo<'env>>,
}

/// Which part of the file to blame.
#[napi(object)]
pub struct BlameOptions {
    /// Blame the file as of this snapshot; the checked-out branch's tip when absent.
    pub at: Option<String>,
    /// First line to attribute, 1-based and inclusive.
    pub start_line: Option<u32>,
    /// Last line to attribute, 1-based and inclusive.
    pub end_line: Option<u32>,
}

struct OriginData {
    id: String,
    created_at_ms: i64,
    message: String,
    author: Option<(String, Option<String>)>,
    branch: String,
    path: String,
}

struct LineData {
    line_no: u32,
    text: String,
    line_count: u32,
    origin: Option<OriginData>,
}

pub struct BlameData {
    path: String,
    snapshot: String,
    lines: Vec<LineData>,
}

pub struct BlameJob {
    repo: Shared,
    file: String,
    at: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

fn blame_data(job: &BlameJob, repo: &velo_core::Repo) -> CoreResult<BlameData> {
    let at = job.at.as_deref().map(snapshot_id).transpose()?;
    // JS counts lines from 1 and includes the end; core's window is half-open.
    let lines = match (job.start_line, job.end_line) {
        (None, None) => None,
        (start, end) => {
            let start = start.unwrap_or(1) as usize;
            let end = end.map_or(usize::MAX, |e| (e as usize).saturating_add(1));
            Some(start..end)
        }
    };
    let b = blame::run(
        repo,
        Path::new(&job.file),
        blame::Options {
            at: at.as_ref(),
            lines,
            ..Default::default()
        },
    )?;
    Ok(BlameData {
        path: b.path.to_string_lossy().into_owned(),
        snapshot: b.snapshot.as_str().to_string(),
        lines: b
            .lines
            .into_iter()
            .map(|l| LineData {
                line_no: l.line_no as u32,
                text: l.text,
                line_count: l.line_count as u32,
                origin: l.origin.map(|o| OriginData {
                    id: o.hash.as_str().to_string(),
                    created_at_ms: o.created_at.timestamp_millis(),
                    message: o.message,
                    author: o
                        .author
                        .map(|a| (a.name().to_string(), a.email().map(str::to_string))),
                    branch: o.branch.as_str().to_string(),
                    path: o.path.to_string_lossy().into_owned(),
                }),
            })
            .collect(),
    })
}

impl<'task> ScopedTask<'task> for BlameJob {
    type Output = CoreResult<BlameData>;
    type JsValue = BlameInfo<'task>;

    fn compute(&mut self) -> Result<Self::Output> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| napi::Error::from_reason("velo Repo is poisoned by an earlier panic"))?;
        Ok(blame_data(self, &repo))
    }

    fn resolve(&mut self, env: &'task Env, output: Self::Output) -> Result<Self::JsValue> {
        let d = output.map_err(|e| to_js(env, e))?;
        let mut lines = Vec::with_capacity(d.lines.len());
        for l in d.lines {
            let origin = match l.origin {
                None => Either::B(Null),
                Some(o) => Either::A(LineOriginInfo {
                    id: o.id,
                    created_at_ms: o.created_at_ms,
                    created_at: env.create_date(o.created_at_ms as f64)?,
                    message: o.message,
                    author: match o.author {
                        Some((name, email)) => Either::A(AuthorInfo {
                            name,
                            email: nullable(email),
                        }),
                        None => Either::B(Null),
                    },
                    branch: o.branch,
                    path: o.path,
                }),
            };
            lines.push(BlameLineInfo {
                line_no: l.line_no,
                text: l.text,
                line_count: l.line_count,
                origin,
            });
        }
        Ok(BlameInfo {
            path: d.path,
            snapshot: d.snapshot,
            lines,
        })
    }
}

#[napi]
impl Repo {
    /// Snapshots newest first, filtered by `options`.
    #[napi(ts_return_type = "Promise<Array<SnapshotInfo>>")]
    pub fn history(&self, options: Option<HistoryOptions>) -> AsyncTask<EntriesJob> {
        let o = options.unwrap_or(HistoryOptions {
            from: None,
            branch: None,
            all: None,
            paths: None,
            limit: None,
            meta: None,
        });
        AsyncTask::new(EntriesJob {
            repo: self.inner.clone(),
            run: Some(Box::new(move |repo| {
                let from = o.from.as_deref().map(snapshot_id).transpose()?;
                let branch = o.branch.as_deref().map(branch_name).transpose()?;
                let paths: Vec<PathBuf> = o
                    .paths
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect();
                let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                let meta = o.meta.unwrap_or_default();
                let meta = filters(&meta);
                let h = history::run(
                    repo,
                    history::Options {
                        all: o.all.unwrap_or(false),
                        branch: branch.as_ref(),
                        from: from.as_ref(),
                        paths: &path_refs,
                        meta: &meta,
                        limit: o.limit.map(|n| n as usize),
                    },
                )?;
                Ok(h.entries
                    .into_iter()
                    .map(SnapshotData::from_entry)
                    .collect())
            })),
        })
    }

    /// Snapshots whose metadata satisfies every filter, newest first.
    #[napi(ts_return_type = "Promise<Array<SnapshotInfo>>")]
    pub fn find_snapshots(&self, meta: Vec<MetaFilterInput>) -> AsyncTask<EntriesJob> {
        AsyncTask::new(EntriesJob {
            repo: self.inner.clone(),
            run: Some(Box::new(move |repo| {
                let found = repo.find_snapshots(&filters(&meta))?;
                Ok(found.into_iter().map(SnapshotData::from_entry).collect())
            })),
        })
    }

    /// Attribute each line of `path` to the snapshot that last changed it.
    #[napi(ts_return_type = "Promise<BlameInfo>")]
    pub fn blame(&self, path: String, options: Option<BlameOptions>) -> AsyncTask<BlameJob> {
        let (at, start_line, end_line) = match options {
            Some(o) => (o.at, o.start_line, o.end_line),
            None => (None, None, None),
        };
        AsyncTask::new(BlameJob {
            repo: self.inner.clone(),
            file: path,
            at,
            start_line,
            end_line,
        })
    }
}
