//! Server side of the streaming sync protocol.
//!
//! These run on the *remote* host (invoked as `velo serve-upload <path>` /
//! `velo serve-receive <path>`, typically over ssh) and speak the binary
//! protocol described in `transport.rs` over stdin/stdout. They must write
//! nothing to stdout except protocol bytes; diagnostics go to stderr.
//!
//! The [`http`] submodule exposes the same operations as plain
//! bytes-in/bytes-out functions, so a service can mount them in any web
//! framework without velo-core depending on one. Both front ends share the
//! private helpers below, so there is one implementation of "which pack does
//! this client need" and "is this push a fast-forward".

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::commands::{all_branch_tips, bundle};
use crate::error::{Result, VeloError};
use crate::transport;

fn require_repo(path: &str) -> Result<PathBuf> {
    let root = PathBuf::from(path);
    check_repo(&root)?;
    Ok(root)
}

fn check_repo(root: &Path) -> Result<()> {
    if root.join(".velo/velo.db").is_file() {
        Ok(())
    } else {
        Err(VeloError::invalid(format!(
            "'{}' is not a Velo repository.",
            root.display()
        )))
    }
}

/// The pack a client holding `have` needs from a repository whose branch tips
/// are `tips`: everything reachable from a tip that the client lacks, minus
/// objects it already holds through the snapshots it reported.
fn pack_for(
    conn: &rusqlite::Connection,
    objects: &crate::storage::ObjectStore,
    tips: &[(String, String)],
    have: &HashSet<String>,
) -> Result<bundle::Bundle> {
    let mut snap_set: HashSet<String> = HashSet::new();
    for (_b, tip) in tips {
        snap_set.extend(bundle::reachable_ancestry(conn, tip));
    }
    for h in have {
        snap_set.remove(h);
    }
    bundle::build_pack_excluding(conn, objects, &snap_set, have)
}

/// Run the fast-forward check and import if it passes, returning the status
/// string the client expects (`OK <snapshots> <objects> <tags>` or `REJECT <reason>`).
fn apply_push(
    guard: &crate::WriteGuard,
    branch: &str,
    new_tip: &str,
    pack: &bundle::Bundle,
) -> Result<String> {
    Ok(
        match transport::fast_forward_check(guard.conn(), branch, new_tip, pack) {
            Some(reason) => format!("REJECT {}", reason),
            None => {
                let (s, o, t) = bundle::import_pack_tags(guard, pack)?;
                guard.repo().emit_imported(s);
                format!("OK {} {} {}", s, o, t)
            }
        },
    )
}

/// Serve a fetch: advertise refs, read the client's "have" set, send a pack of
/// everything reachable from our tips that the client lacks.
pub fn upload(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    let repo = crate::Repo::open_and_migrate(&root)?;
    let conn = repo.conn();

    let stdout = io::stdout();
    let mut out = stdout.lock();

    let tips = all_branch_tips(conn);
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // Read the client's have-set until EOF.
    let stdin = io::stdin();
    let mut inp = stdin.lock();
    let mut have: HashSet<String> = HashSet::new();
    let max_string = repo.limits().max_string_bytes;
    while let Some(h) = transport::read_string_opt(&mut inp, max_string)? {
        have.insert(h);
    }

    let pack = pack_for(conn, &repo.objects(), &tips, &have)?;
    out.write_all(&bundle::encode(&pack))
        .map_err(VeloError::Io)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}

/// Serve a push: advertise refs, read (branch, new_tip, pack), run the
/// fast-forward check, import if it passes, and report status.
pub fn receive(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    let repo = crate::Repo::open_and_migrate(&root)?;
    // Hold the repo lock for the whole exchange so the push is atomic against
    // other velo processes on this host. The guard is also what lets us import.
    let guard = repo.write()?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stdin = io::stdin();
    let mut inp = stdin.lock();

    // S→C refs.
    let tips = all_branch_tips(guard.conn());
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // C→S branch, new_tip, then the pack to EOF.
    let limits = guard.repo().limits();
    let branch = transport::read_string(&mut inp, limits.max_string_bytes)?;
    let new_tip = transport::read_string(&mut inp, limits.max_string_bytes)?;
    // One byte past the ceiling is the proof of overrun; stdin is the sender's
    // to make as long as it likes.
    let mut packbytes = Vec::new();
    (&mut inp)
        .take(limits.max_pack_bytes.saturating_add(1))
        .read_to_end(&mut packbytes)
        .map_err(VeloError::Io)?;
    if packbytes.len() as u64 > limits.max_pack_bytes {
        return Err(VeloError::LimitExceeded {
            limit: crate::limits::Limit::PackBytes,
            max: limits.max_pack_bytes,
            found: None,
        });
    }
    let pack = bundle::decode(&packbytes, limits)?;

    let status = apply_push(&guard, &branch, &new_tip, &pack)?;
    transport::write_string(&mut out, &status)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}

/// The server side of the HTTP transport, as framework-agnostic handlers.
///
/// Each function is one endpoint: bytes in, bytes out, no sockets and no HTTP
/// library. A hosting service mounts them under any prefix it likes:
///
/// | Method | Path               | Handler           |
/// |--------|--------------------|-------------------|
/// | GET    | `/velo/v1/refs`    | [`http::refs`]    |
/// | POST   | `/velo/v1/upload`  | [`http::upload`]  |
/// | POST   | `/velo/v1/receive` | [`http::receive`] |
///
/// An `Err` is a server-side failure (map it to a 4xx/5xx); a refused push is
/// *not* an error, it is an `Ok` carrying a `REJECT` status, exactly as over
/// ssh. The three functions do no authentication; [`http::Service`] wraps them
/// with routing, bearer-token authorization, read-only mode and request limits.
pub mod http {
    use std::collections::HashSet;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    use super::{apply_push, check_repo, pack_for};
    use crate::commands::{all_branch_tips, bundle};
    use crate::error::{Result, VeloError};
    use crate::transport;
    use crate::Limits;

    /// `GET /velo/v1/refs`: the branch tips, as a refs block.
    pub fn refs(root: &Path) -> Result<Vec<u8>> {
        refs_with(root, &Limits::default())
    }

    fn refs_with(root: &Path, limits: &Limits) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?.limited(limits.clone());
        let mut out = Vec::new();
        transport::write_refs(&mut out, &all_branch_tips(repo.conn()))?;
        Ok(out)
    }

    /// `POST /velo/v1/upload`: serve a fetch.
    ///
    /// `body` is the client's have-ids as length-prefixed strings. The response
    /// is a refs block followed by the pack.
    pub fn upload(root: &Path, body: &[u8]) -> Result<Vec<u8>> {
        upload_with(root, body, &Limits::default())
    }

    fn upload_with(root: &Path, body: &[u8], limits: &Limits) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?.limited(limits.clone());
        let conn = repo.conn();

        let mut inp = Cursor::new(body);
        let mut have: HashSet<String> = HashSet::new();
        let max_string = repo.limits().max_string_bytes;
        while let Some(h) = transport::read_string_opt(&mut inp, max_string)? {
            have.insert(h);
        }

        let tips = all_branch_tips(conn);
        let mut out = Vec::new();
        transport::write_refs(&mut out, &tips)?;
        let pack = pack_for(conn, &repo.objects(), &tips, &have)?;
        out.extend_from_slice(&bundle::encode(&pack));
        Ok(out)
    }

    /// `POST /velo/v1/receive`: serve a push.
    ///
    /// `body` is the branch and new tip (length-prefixed strings) followed by
    /// the pack. The response is a length-prefixed status string, `OK n m` or
    /// `REJECT reason`. The repository write lock is held throughout.
    pub fn receive(root: &Path, body: &[u8]) -> Result<Vec<u8>> {
        receive_with(root, body, &Limits::default())
    }

    fn receive_with(root: &Path, body: &[u8], limits: &Limits) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?.limited(limits.clone());
        let guard = repo.write()?;

        let mut inp = Cursor::new(body);
        let limits = guard.repo().limits();
        let branch = transport::read_string(&mut inp, limits.max_string_bytes)?;
        let new_tip = transport::read_string(&mut inp, limits.max_string_bytes)?;
        let at = inp.position() as usize;
        let pack = bundle::decode(&body[at..], limits)?;

        let status = apply_push(&guard, &branch, &new_tip, &pack)?;
        let mut out = Vec::new();
        transport::write_string(&mut out, &status)?;
        Ok(out)
    }

    /// Refusal kind: the repository path is unsafe or malformed.
    pub const BAD_PATH: &str = "bad-path";
    /// Refusal kind: no route, or no such repository.
    pub const NOT_FOUND: &str = "not-found";
    /// Refusal kind: the route exists but not for this method.
    pub const METHOD_NOT_ALLOWED: &str = "method-not-allowed";
    /// Refusal kind: no valid bearer token was presented.
    pub const UNAUTHENTICATED: &str = "unauthenticated";
    /// Refusal kind: the token is valid but does not allow this operation.
    pub const FORBIDDEN: &str = "forbidden";
    /// Refusal kind: the service is read-only and this is a push.
    pub const READ_ONLY: &str = "read-only";
    /// Refusal kind: the request or an object in it exceeds a limit.
    pub const TOO_LARGE: &str = "too-large";
    /// Refusal kind: the repository is locked by another writer.
    pub const BUSY: &str = "busy";
    /// Refusal kind: any other server-side failure.
    pub const ERROR: &str = "error";

    /// What a request asks to do.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Operation {
        /// `GET refs`.
        Refs,
        /// `POST upload`: a fetch.
        Upload,
        /// `POST receive`: a push.
        Receive,
    }

    impl Operation {
        /// Whether the operation changes the repository.
        pub fn is_write(self) -> bool {
            matches!(self, Operation::Receive)
        }
    }

    /// An authorizer's verdict.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Access {
        /// Proceed.
        Allow,
        /// No valid credentials: the wire answer is 401.
        Unauthenticated,
        /// Valid credentials that do not cover this: the wire answer is 403.
        Forbidden,
    }

    /// A host-supplied access check. An embedder serving many repositories
    /// decides here who may read or write which. `repository` is the
    /// repository path as addressed in the URL (empty for a single-repository
    /// service with no prefix).
    pub trait Authorize: Send + Sync {
        /// Decide whether `bearer` may perform `operation` on `repository`.
        fn authorize(&self, repository: &str, operation: Operation, bearer: Option<&str>)
            -> Access;
    }

    /// Constant-time byte equality: XOR-folds over the longer length with the
    /// length difference folded in, and never returns early, so timing does
    /// not reveal how much of a token matched.
    pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        let mut diff = (a.len() ^ b.len()) as u64;
        let n = a.len().max(b.len());
        for i in 0..n {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            diff |= u64::from(x ^ y);
        }
        diff == 0
    }

    /// A set of bearer tokens: read tokens may fetch, write tokens may also
    /// push. Every configured token is compared on every check.
    #[derive(Clone, Debug, Default)]
    pub struct Tokens {
        read: Vec<Vec<u8>>,
        write: Vec<Vec<u8>>,
    }

    impl Tokens {
        /// No tokens: everything is refused as unauthenticated.
        pub fn new() -> Self {
            Self::default()
        }

        /// Add a token that may fetch.
        pub fn read(mut self, token: impl Into<String>) -> Self {
            self.read.push(token.into().into_bytes());
            self
        }

        /// Add a token that may fetch and push.
        pub fn write(mut self, token: impl Into<String>) -> Self {
            self.write.push(token.into().into_bytes());
            self
        }
    }

    impl Authorize for Tokens {
        fn authorize(
            &self,
            _repository: &str,
            operation: Operation,
            bearer: Option<&str>,
        ) -> Access {
            let Some(bearer) = bearer else {
                return Access::Unauthenticated;
            };
            let presented = bearer.as_bytes();
            // No short-circuit: every token is compared whatever the outcome.
            let mut is_write = false;
            let mut is_read = false;
            for t in &self.write {
                is_write |= ct_eq(presented, t);
            }
            for t in &self.read {
                is_read |= ct_eq(presented, t);
            }
            if is_write || (is_read && !operation.is_write()) {
                Access::Allow
            } else if is_read {
                Access::Forbidden
            } else {
                Access::Unauthenticated
            }
        }
    }

    /// One HTTP request, as the front end parsed it.
    #[derive(Clone, Copy, Debug)]
    #[non_exhaustive]
    pub struct Request<'a> {
        /// `GET` or `POST`.
        pub method: &'a str,
        /// The request target, query string allowed.
        pub path: &'a str,
        /// The raw `Authorization` header value, if any.
        pub authorization: Option<&'a str>,
        /// The request body.
        pub body: &'a [u8],
    }

    impl<'a> Request<'a> {
        /// Build a request.
        pub fn new(
            method: &'a str,
            path: &'a str,
            authorization: Option<&'a str>,
            body: &'a [u8],
        ) -> Self {
            Request {
                method,
                path,
                authorization,
                body,
            }
        }
    }

    /// The answer to send back.
    #[derive(Clone, Debug)]
    #[non_exhaustive]
    pub struct Response {
        /// HTTP status code.
        pub status: u16,
        /// `Content-Type` value.
        pub content_type: &'static str,
        /// Response body.
        pub body: Vec<u8>,
        /// Whether to send `WWW-Authenticate: Bearer`.
        pub www_authenticate: bool,
    }

    const BINARY: &str = "application/octet-stream";
    const TEXT: &str = "text/plain";

    fn refusal(status: u16, kind: &str, detail: &str) -> Response {
        Response {
            status,
            content_type: TEXT,
            body: format!("velo: {kind}: {detail}\n").into_bytes(),
            www_authenticate: false,
        }
    }

    enum Mode {
        Single,
        Directory,
    }

    /// A framework-agnostic HTTP service over one repository or a directory of
    /// them: routing, authorization, read-only and request limits.
    pub struct Service {
        root: PathBuf,
        mode: Mode,
        authorizer: Option<Box<dyn Authorize>>,
        read_only: bool,
        limits: Limits,
    }

    impl Service {
        fn new(root: impl Into<PathBuf>, mode: Mode) -> Self {
            Service {
                root: root.into(),
                mode,
                authorizer: None,
                read_only: false,
                limits: Limits::default(),
            }
        }

        /// Serve one repository at `<any prefix>/velo/v1/...`.
        pub fn single(root: impl Into<PathBuf>) -> Self {
            Self::new(root, Mode::Single)
        }

        /// Serve every repository under `root`, at `/<repo path>/velo/v1/...`.
        pub fn directory(root: impl Into<PathBuf>) -> Self {
            Self::new(root, Mode::Directory)
        }

        /// Check every request with `a`. Without one, everything is allowed.
        pub fn authorize_with(mut self, a: impl Authorize + 'static) -> Self {
            self.authorizer = Some(Box::new(a));
            self
        }

        /// Serve fetch and refuse push.
        pub fn read_only(mut self, yes: bool) -> Self {
            self.read_only = yes;
            self
        }

        /// Read limits for requests and received objects.
        pub fn limits(mut self, limits: Limits) -> Self {
            self.limits = limits;
            self
        }

        /// The largest request body accepted. Front ends check
        /// `Content-Length` against this before reading the body.
        pub fn max_request_bytes(&self) -> u64 {
            self.limits.max_pack_bytes
        }

        /// Answer one request.
        pub fn handle(&self, req: &Request<'_>) -> Response {
            let path = req.path.split('?').next().unwrap_or("");
            let (suffix, op, method) = if path.ends_with("/velo/v1/refs") {
                ("/velo/v1/refs", Operation::Refs, "GET")
            } else if path.ends_with("/velo/v1/upload") {
                ("/velo/v1/upload", Operation::Upload, "POST")
            } else if path.ends_with("/velo/v1/receive") {
                ("/velo/v1/receive", Operation::Receive, "POST")
            } else {
                return refusal(404, NOT_FOUND, "no such endpoint");
            };
            if req.method != method {
                return refusal(405, METHOD_NOT_ALLOWED, &format!("use {method}"));
            }
            let prefix = &path[..path.len() - suffix.len()];

            let (repo_name, dir) = match self.resolve(prefix) {
                Ok(r) => r,
                Err(resp) => return resp,
            };

            if let Some(auth) = &self.authorizer {
                let bearer = req.authorization.and_then(parse_bearer);
                match auth.authorize(&repo_name, op, bearer) {
                    Access::Allow => {}
                    Access::Forbidden => {
                        return refusal(403, FORBIDDEN, "this token may not do that");
                    }
                    // Unknown future verdicts fail closed as unauthenticated.
                    _ => {
                        let mut r = refusal(401, UNAUTHENTICATED, "a bearer token is required");
                        r.www_authenticate = true;
                        return r;
                    }
                }
            }
            if self.read_only && op.is_write() {
                return refusal(403, READ_ONLY, "this service does not accept pushes");
            }
            if req.body.len() as u64 > self.max_request_bytes() {
                return refusal(
                    413,
                    TOO_LARGE,
                    &format!("request body exceeds {} bytes", self.max_request_bytes()),
                );
            }

            let out = match op {
                Operation::Refs => refs_with(&dir, &self.limits),
                Operation::Upload => upload_with(&dir, req.body, &self.limits),
                Operation::Receive => receive_with(&dir, req.body, &self.limits),
            };
            match out {
                Ok(body) => Response {
                    status: 200,
                    content_type: BINARY,
                    body,
                    www_authenticate: false,
                },
                Err(VeloError::LimitExceeded { .. }) => {
                    refusal(413, TOO_LARGE, "received data exceeds a configured limit")
                }
                Err(VeloError::Locked { .. }) => {
                    refusal(503, BUSY, "the repository is locked by another writer")
                }
                Err(_) => refusal(500, ERROR, "the request could not be served"),
            }
        }

        /// The repository name (as addressed) and its directory, or the
        /// refusal to send.
        fn resolve(&self, prefix: &str) -> std::result::Result<(String, PathBuf), Response> {
            let not_found = || refusal(404, NOT_FOUND, "no such repository");
            let (name, dir) = match self.mode {
                Mode::Single => (
                    prefix.trim_start_matches('/').to_string(),
                    self.root.clone(),
                ),
                Mode::Directory => {
                    if prefix.contains('%') {
                        return Err(refusal(400, BAD_PATH, "percent-encoding is not accepted"));
                    }
                    if prefix.contains('\\') {
                        return Err(refusal(400, BAD_PATH, "backslash in repository path"));
                    }
                    let name = prefix.strip_prefix('/').unwrap_or(prefix);
                    if name.is_empty() {
                        return Err(not_found());
                    }
                    for seg in name.split('/') {
                        if seg.is_empty()
                            || seg == "."
                            || seg == ".."
                            || crate::safe_path::check(seg).is_err()
                        {
                            return Err(refusal(400, BAD_PATH, "unsafe repository path"));
                        }
                    }
                    let mut dir = self.root.clone();
                    dir.extend(name.split('/'));
                    (name.to_string(), dir)
                }
            };
            if !dir.join(".velo/velo.db").is_file() {
                return Err(not_found());
            }
            if matches!(self.mode, Mode::Directory) {
                let canon = dir.canonicalize().map_err(|_| not_found())?;
                let root = self.root.canonicalize().map_err(|_| not_found())?;
                if !canon.starts_with(&root) {
                    return Err(not_found());
                }
            }
            Ok((name, dir))
        }
    }

    /// The token of an `Authorization: Bearer <token>` header; the scheme is
    /// case-insensitive.
    fn parse_bearer(header: &str) -> Option<&str> {
        let (scheme, rest) = header.split_once(' ')?;
        let token = rest.trim();
        (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
    }
}
