//! Snapshot signing and one-step sync.
//!
//! A second `#[pymethods]` block on `Repo`. The private key lives in a
//! `SigningKey` object the caller holds; velody never stores it and `repr` shows
//! only the public half. Trust is a list of public keys given per `verify` call:
//! velody does not own that decision.

use std::path::PathBuf;

use pyo3::prelude::*;
use velody_core::commands::sync::{self, Pushed, SyncOptions, Synced};
use velody_core::sign::{self, PublicKey, TrustedKeys};
use velody_core::transport::Spawn;

use crate::errors::to_py;
use crate::repo::{snapshot_id, Author, Repo};

/// A private Ed25519 signing key.
#[pyclass(frozen, skip_from_py_object, module = "velody")]
#[derive(Clone)]
pub struct SigningKey {
    inner: sign::SigningKey,
}

#[pymethods]
impl SigningKey {
    /// Build a key from its 32-byte seed.
    #[staticmethod]
    fn from_seed(seed: &[u8]) -> PyResult<Self> {
        let seed: [u8; 32] = seed.try_into().map_err(|_| {
            to_py(velody_core::Error::invalid(
                "a signing key seed is exactly 32 bytes.",
            ))
        })?;
        Ok(SigningKey {
            inner: sign::SigningKey::from_seed(seed),
        })
    }

    /// Build a key from its seed written as 64 hex characters.
    #[staticmethod]
    fn from_hex(text: &str) -> PyResult<Self> {
        Ok(SigningKey {
            inner: sign::SigningKey::from_hex(text).map_err(to_py)?,
        })
    }

    /// The public key as hex: what `verify` takes as a trusted key.
    #[getter]
    fn public_key(&self) -> String {
        self.inner.public_key().to_string()
    }

    fn __repr__(&self) -> String {
        format!("SigningKey(public: {})", self.inner.public_key())
    }
}

/// One signature on a snapshot, as stored.
#[pyclass(frozen, get_all, module = "velody")]
pub struct Signature {
    snapshot: String,
    algorithm: String,
    public_key: String,
    signature: String,
}

#[pymethods]
impl Signature {
    fn __repr__(&self) -> String {
        format!("Signature({}, {})", self.snapshot, self.public_key)
    }
}

impl From<sign::Signature> for Signature {
    fn from(s: sign::Signature) -> Self {
        Signature {
            snapshot: s.snapshot.as_str().to_string(),
            algorithm: s.algorithm,
            public_key: s.public_key,
            signature: s.signature,
        }
    }
}

/// Every signature on a snapshot, sorted by outcome. Each list holds public
/// keys as hex.
#[pyclass(frozen, get_all, module = "velody")]
pub struct Verification {
    snapshot: String,
    trusted: Vec<String>,
    untrusted: Vec<String>,
    invalid: Vec<String>,
    unverifiable: Vec<String>,
}

#[pymethods]
impl Verification {
    /// True when at least one valid signature is by a trusted key and none is invalid.
    #[getter]
    fn is_trusted(&self) -> bool {
        !self.trusted.is_empty() && self.invalid.is_empty()
    }

    fn __repr__(&self) -> String {
        format!(
            "Verification({}, trusted={}, untrusted={}, invalid={}, unverifiable={})",
            self.snapshot,
            self.trusted.len(),
            self.untrusted.len(),
            self.invalid.len(),
            self.unverifiable.len()
        )
    }
}

/// What a push did.
#[pyclass(frozen, get_all, module = "velody")]
pub struct PushReport {
    /// `"sent"` or `"already_up_to_date"`.
    outcome: String,
    branch: String,
    remote: String,
    snapshots: usize,
    objects: usize,
    /// `"remote_was_empty"` or `"branch_was_missing"` when the push created the branch.
    created: Option<String>,
    also_published: Vec<String>,
}

#[pymethods]
impl PushReport {
    fn __repr__(&self) -> String {
        format!(
            "PushReport({}, {}/{})",
            self.outcome, self.remote, self.branch
        )
    }
}

fn push_report(r: sync::PushReport) -> PushReport {
    let also_published = r.also_published;
    match r.pushed {
        Pushed::AlreadyUpToDate { branch, remote } => PushReport {
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
        } => PushReport {
            outcome: "sent".into(),
            branch,
            remote,
            snapshots,
            objects,
            created: created.map(|c| {
                match c {
                    sync::BranchCreated::RemoteWasEmpty => "remote_was_empty",
                    sync::BranchCreated::BranchWasMissing => "branch_was_missing",
                }
                .to_string()
            }),
            also_published,
        },
    }
}

/// What `Repo.sync` did.
#[pyclass(frozen, get_all, module = "velody")]
pub struct SyncResult {
    /// `"nothing"`, `"pushed"`, `"fast_forwarded"`, `"merged"` or `"conflicted"`.
    kind: String,
    branch: String,
    remote: String,
    /// The snapshot the branch moved to (`fast_forwarded`).
    to: Option<String>,
    /// The merge snapshot (`merged`).
    merge: Option<String>,
    /// The conflicted paths (`conflicted`).
    paths: Option<Vec<String>>,
    /// The push (`pushed`, `merged`).
    report: Option<Py<PushReport>>,
}

#[pymethods]
impl SyncResult {
    fn __repr__(&self) -> String {
        format!("SyncResult({}, {}/{})", self.kind, self.remote, self.branch)
    }
}

fn sync_result(py: Python<'_>, synced: Synced) -> PyResult<SyncResult> {
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
            out.report = Some(Py::new(py, report)?);
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
            out.report = Some(Py::new(py, push_report(report))?);
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
            return Err(to_py(velody_core::Error::invalid(
                "sync returned an outcome this binding does not know.",
            )))
        }
    }
    Ok(out)
}

#[pymethods]
impl Repo {
    /// Sign `snapshot` with `key`; the signature is stored beside the snapshot.
    fn sign(&self, py: Python<'_>, snapshot: &str, key: &SigningKey) -> PyResult<Signature> {
        let id = snapshot_id(snapshot)?;
        self.check_owner()?;
        let signed = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            let guard = match repo.write() {
                Ok(g) => g,
                Err(e) => return Ok(Err(e)),
            };
            Ok(sign::sign(&guard, &id, &key.inner))
        })?;
        signed.map(Signature::from).map_err(to_py)
    }

    /// Every signature stored for `snapshot`, valid or not.
    fn signatures(&self, snapshot: &str) -> PyResult<Vec<Signature>> {
        let id = snapshot_id(snapshot)?;
        let repo = self.core()?;
        Ok(sign::signatures(&repo, &id)
            .map_err(to_py)?
            .into_iter()
            .map(Signature::from)
            .collect())
    }

    /// Check the signatures on `snapshot` against `trusted_keys` (hex public keys).
    fn verify(
        &self,
        py: Python<'_>,
        snapshot: &str,
        trusted_keys: Vec<String>,
    ) -> PyResult<Verification> {
        let id = snapshot_id(snapshot)?;
        let trusted: TrustedKeys = trusted_keys
            .iter()
            .map(|k| k.parse::<PublicKey>())
            .collect::<Result<_, _>>()
            .map_err(to_py)?;
        self.check_owner()?;
        let v = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            Ok(sign::verify(&repo, &id, &trusted))
        })?;
        let v = v.map_err(to_py)?;
        Ok(Verification {
            snapshot: v.snapshot.as_str().to_string(),
            trusted: v.trusted,
            untrusted: v.untrusted,
            invalid: v.invalid,
            unverifiable: v.unverifiable,
        })
    }

    /// The ids of snapshots that carry at least one signature.
    fn signed_snapshots(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        self.check_owner()?;
        let found = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            Ok(sign::signed_snapshots(&repo))
        })?;
        let mut ids: Vec<String> = found
            .map_err(to_py)?
            .into_iter()
            .map(|s| s.as_str().to_string())
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Fetch, then push, fast-forward or merge as needed.
    ///
    /// A merge is saved and pushed only when clean; a conflicted one stops as
    /// `merge` leaves it and nothing is pushed. `token` is a bearer token for an
    /// HTTP remote, supplied for this call only and never included in an error.
    /// HTTP and local-path remotes need no `velody_bin`; `child:` and `ssh://`
    /// remotes do, because a binding has no `velody` executable of its own (it
    /// defaults to `velody` on the path). Needs a working tree.
    #[pyo3(signature = (remote = "origin", branch = None, token = None, author = None,
        message = None, velody_bin = None))]
    #[allow(clippy::too_many_arguments)]
    fn sync(
        &self,
        py: Python<'_>,
        remote: &str,
        branch: Option<&str>,
        token: Option<String>,
        author: Option<PyRef<'_, Author>>,
        message: Option<&str>,
        velody_bin: Option<PathBuf>,
    ) -> PyResult<SyncResult> {
        let spawn = Spawn::new(velody_bin.unwrap_or_else(|| PathBuf::from("velody")));
        let mut options = SyncOptions::default();
        if let Some(a) = author.as_ref() {
            options = options.author(&a.inner);
        }
        if let Some(m) = message {
            options = options.message(m);
        }
        self.check_owner()?;
        // `credentials` consumes a handle, so a token goes to a temporary one
        // opened like this one (same listener and merge drivers); the shared
        // handle is then left unlocked.
        let credentialed = match token {
            Some(token) => {
                let (drivers, _) = self.hooks.drivers_with(py, None).map_err(to_py)?;
                Some(
                    velody_core::Repo::open_and_migrate(&self.root)
                        .map_err(to_py)?
                        .listening(self.hooks.listener())
                        .merging(drivers)
                        .credentials(move |_url| Some(token.clone())),
                )
            }
            None => None,
        };
        let synced = py.detach(move || -> PyResult<_> {
            let shared;
            let target: &velody_core::Repo = match &credentialed {
                Some(own) => own,
                None => {
                    shared = self.core()?;
                    &shared
                }
            };
            let guard = match target.write() {
                Ok(g) => g,
                Err(e) => return Ok(Err(e)),
            };
            let synced = sync::sync(&guard, remote, branch, &spawn, options);
            drop(guard);
            Ok(synced)
        })?;
        sync_result(py, synced.map_err(to_py)?)
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SigningKey>()?;
    m.add_class::<Signature>()?;
    m.add_class::<Verification>()?;
    m.add_class::<PushReport>()?;
    m.add_class::<SyncResult>()?;
    Ok(())
}
