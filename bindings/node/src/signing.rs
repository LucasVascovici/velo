//! Snapshot signing and one-step sync.
//!
//! A second `#[napi] impl Repo` block. The private key lives in a `SigningKey`
//! the caller holds; velody never stores it. Trust is a list of public keys given
//! per `verify` call: velody does not own that decision.

use std::path::PathBuf;

use napi::bindgen_prelude::*;
use napi::Env;
use napi_derive::napi;
use velody_core::commands::sync::{self, BranchCreated, Pushed, SyncOptions, Synced};
use velody_core::sign::{self, PublicKey, TrustedKeys};
use velody_core::transport::Spawn;
use velody_core::Error as CoreError;

use crate::errors::to_js;
use crate::repo::{build_author, snapshot_id, AuthorInput, CoreResult, Job, Repo};

/// A private Ed25519 signing key.
#[napi]
pub struct SigningKey {
    inner: sign::SigningKey,
}

#[napi]
impl SigningKey {
    /// Build a key from its 32-byte seed.
    #[napi(factory)]
    pub fn from_seed(env: &Env, seed: Buffer) -> Result<Self> {
        let seed: [u8; 32] = seed.as_ref().try_into().map_err(|_| {
            to_js(
                env,
                CoreError::invalid("a signing key seed is exactly 32 bytes."),
            )
        })?;
        Ok(SigningKey {
            inner: sign::SigningKey::from_seed(seed),
        })
    }

    /// Build a key from its seed written as 64 hex characters.
    #[napi(factory)]
    pub fn from_hex(env: &Env, text: String) -> Result<Self> {
        Ok(SigningKey {
            inner: sign::SigningKey::from_hex(&text).map_err(|e| to_js(env, e))?,
        })
    }

    /// The public key as hex: what `verify` takes as a trusted key.
    #[napi(getter)]
    pub fn public_key(&self) -> String {
        self.inner.public_key().to_string()
    }
}

/// One signature on a snapshot, as stored.
#[napi(object)]
pub struct SignatureInfo {
    pub snapshot: String,
    pub algorithm: String,
    pub public_key: String,
    pub signature: String,
}

impl From<sign::Signature> for SignatureInfo {
    fn from(s: sign::Signature) -> Self {
        SignatureInfo {
            snapshot: s.snapshot.as_str().to_string(),
            algorithm: s.algorithm,
            public_key: s.public_key,
            signature: s.signature,
        }
    }
}

/// Every signature on a snapshot, sorted by outcome. Each list holds public
/// keys as hex.
#[napi(object)]
pub struct VerificationInfo {
    pub snapshot: String,
    pub trusted: Vec<String>,
    pub untrusted: Vec<String>,
    pub invalid: Vec<String>,
    pub unverifiable: Vec<String>,
    /// True when at least one valid signature is by a trusted key and none is invalid.
    pub is_trusted: bool,
}

/// What a push did.
#[napi(object)]
pub struct PushReportInfo {
    /// `'sent'` or `'already_up_to_date'`.
    pub outcome: String,
    pub branch: String,
    pub remote: String,
    pub snapshots: u32,
    pub objects: u32,
    /// `'remote_was_empty'` or `'branch_was_missing'` when the push created the branch.
    pub created: Option<String>,
    pub also_published: Vec<String>,
}

fn push_report(r: sync::PushReport) -> PushReportInfo {
    let also_published = r.also_published;
    match r.pushed {
        Pushed::AlreadyUpToDate { branch, remote } => PushReportInfo {
            outcome: "already_up_to_date".into(),
            branch,
            remote,
            snapshots: 0,
            objects: 0,
            created: None,
            also_published,
        },
        Pushed::Sent {
            branch,
            remote,
            snapshots,
            objects,
            created,
        } => PushReportInfo {
            outcome: "sent".into(),
            branch,
            remote,
            snapshots: snapshots as u32,
            objects: objects as u32,
            created: created.map(|c| {
                match c {
                    BranchCreated::RemoteWasEmpty => "remote_was_empty",
                    BranchCreated::BranchWasMissing => "branch_was_missing",
                }
                .to_string()
            }),
            also_published,
        },
    }
}

/// What `sync` did.
#[napi(object)]
pub struct SyncResult {
    /// `'nothing'`, `'pushed'`, `'fast_forwarded'`, `'merged'` or `'conflicted'`.
    pub kind: String,
    pub branch: String,
    pub remote: String,
    /// The snapshot the branch moved to (`fast_forwarded`).
    pub to: Option<String>,
    /// The merge snapshot (`merged`).
    pub merge: Option<String>,
    /// The conflicted paths (`conflicted`).
    pub paths: Option<Vec<String>>,
    /// The push (`pushed`, `merged`).
    pub report: Option<PushReportInfo>,
}

fn sync_result(synced: Synced) -> CoreResult<SyncResult> {
    let mut out = SyncResult {
        kind: String::new(),
        branch: String::new(),
        remote: String::new(),
        to: None,
        merge: None,
        paths: None,
        report: None,
    };
    match synced {
        Synced::Nothing { branch, remote } => {
            out.kind = "nothing".into();
            out.branch = branch;
            out.remote = remote;
        }
        Synced::Pushed { report } => {
            let report = push_report(report);
            out.kind = "pushed".into();
            out.branch = report.branch.clone();
            out.remote = report.remote.clone();
            out.report = Some(report);
        }
        Synced::FastForwarded { branch, remote, to } => {
            out.kind = "fast_forwarded".into();
            out.branch = branch;
            out.remote = remote;
            out.to = Some(to.as_str().to_string());
        }
        Synced::Merged {
            branch,
            remote,
            merge,
            report,
        } => {
            out.kind = "merged".into();
            out.branch = branch;
            out.remote = remote;
            out.merge = Some(merge.as_str().to_string());
            out.report = Some(push_report(report));
        }
        Synced::Conflicted {
            branch,
            remote,
            paths,
        } => {
            out.kind = "conflicted".into();
            out.branch = branch;
            out.remote = remote;
            out.paths = Some(
                paths
                    .iter()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .collect(),
            );
        }
        _ => {
            return Err(CoreError::invalid(
                "sync returned an outcome this binding does not know.",
            ))
        }
    }
    Ok(out)
}

/// What `sync` takes. Every field is optional.
#[napi(object)]
pub struct SyncInput {
    /// The remote's name. Defaults to `'origin'`.
    pub remote: Option<String>,
    /// Must be the checked-out branch; defaults to it.
    pub branch: Option<String>,
    /// A bearer token for an HTTP remote, used for this call only and never
    /// included in an error.
    pub token: Option<String>,
    /// Recorded on a merge snapshot the sync creates.
    pub author: Option<AuthorInput>,
    /// The merge snapshot's message.
    pub message: Option<String>,
    /// The `velody` executable for `child:` and `ssh://` remotes, which a binding
    /// does not have of its own. Defaults to `velody` on the path. HTTP and
    /// local-path remotes need none.
    pub velody_bin: Option<String>,
}

#[napi]
impl Repo {
    /// Sign `snapshot` with `key`; the signature is stored beside the snapshot.
    #[napi(ts_return_type = "Promise<SignatureInfo>")]
    pub fn sign(&self, snapshot: String, key: &SigningKey) -> AsyncTask<Job<SignatureInfo>> {
        let key = key.inner.clone();
        self.job(move |repo| {
            let id = snapshot_id(&snapshot)?;
            let guard = repo.write()?;
            let sig = sign::sign(&guard, &id, &key)?;
            drop(guard);
            Ok(sig.into())
        })
    }

    /// Every signature stored for `snapshot`, valid or not.
    #[napi(ts_return_type = "Promise<Array<SignatureInfo>>")]
    pub fn signatures(&self, snapshot: String) -> AsyncTask<Job<Vec<SignatureInfo>>> {
        self.job(move |repo| {
            let id = snapshot_id(&snapshot)?;
            Ok(sign::signatures(repo, &id)?
                .into_iter()
                .map(SignatureInfo::from)
                .collect())
        })
    }

    /// Check the signatures on `snapshot` against `trustedKeys` (hex public keys).
    #[napi(ts_return_type = "Promise<VerificationInfo>")]
    pub fn verify(
        &self,
        snapshot: String,
        trusted_keys: Vec<String>,
    ) -> AsyncTask<Job<VerificationInfo>> {
        self.job(move |repo| {
            let id = snapshot_id(&snapshot)?;
            let trusted: TrustedKeys = trusted_keys
                .iter()
                .map(|k| k.parse::<PublicKey>())
                .collect::<CoreResult<_>>()?;
            let v = sign::verify(repo, &id, &trusted)?;
            let is_trusted = v.is_trusted();
            Ok(VerificationInfo {
                snapshot: v.snapshot.as_str().to_string(),
                trusted: v.trusted,
                untrusted: v.untrusted,
                invalid: v.invalid,
                unverifiable: v.unverifiable,
                is_trusted,
            })
        })
    }

    /// The ids of snapshots that carry at least one signature.
    #[napi(ts_return_type = "Promise<Array<string>>")]
    pub fn signed_snapshots(&self) -> AsyncTask<Job<Vec<String>>> {
        self.job(|repo| {
            let mut ids: Vec<String> = sign::signed_snapshots(repo)?
                .into_iter()
                .map(|s| s.as_str().to_string())
                .collect();
            ids.sort();
            Ok(ids)
        })
    }

    /// Fetch, then push, fast-forward or merge as needed.
    ///
    /// A merge is saved and pushed only when clean; a conflicted one stops as
    /// `merge` leaves it and nothing is pushed. Needs a working tree.
    #[napi(
        ts_args_type = "options?: SyncInput",
        ts_return_type = "Promise<SyncResult>"
    )]
    pub fn sync(&self, options: Option<SyncInput>) -> AsyncTask<Job<SyncResult>> {
        let SyncInput {
            remote,
            branch,
            token,
            author,
            message,
            velody_bin,
        } = options.unwrap_or(SyncInput {
            remote: None,
            branch: None,
            token: None,
            author: None,
            message: None,
            velody_bin: None,
        });
        let hooks = self.hooks.clone();
        self.job(move |repo| {
            let remote = remote.as_deref().unwrap_or("origin");
            let spawn = Spawn::new(
                velody_bin
                    .map(PathBuf::from)
                    .unwrap_or_else(|| "velody".into()),
            );
            let author = build_author(author)?;
            let mut opts = SyncOptions::default();
            if let Some(a) = author.as_ref() {
                opts = opts.author(a);
            }
            if let Some(m) = message.as_deref() {
                opts = opts.message(m);
            }
            // `credentials` consumes a handle, so a token goes to a temporary
            // one opened like this one (same listener and merge drivers). The
            // shared handle is idle: the job holds its lock.
            let credentialed;
            let target: &velody_core::Repo = match token {
                Some(token) => {
                    let (drivers, _) = hooks.drivers_with(None)?;
                    credentialed = velody_core::Repo::open_and_migrate(&hooks.root)?
                        .listening(hooks.listener())
                        .merging(drivers)
                        .credentials(move |_url| Some(token.clone()));
                    &credentialed
                }
                None => repo,
            };
            let guard = target.write()?;
            let synced = sync::sync(&guard, remote, branch.as_deref(), &spawn, opts)?;
            drop(guard);
            sync_result(synced)
        })
    }
}
