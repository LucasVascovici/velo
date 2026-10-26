# Threat model

What velody defends against, what it does not, and where the known gaps are.
To report a vulnerability see [SECURITY.md](../SECURITY.md).

## Assets

- **History integrity**: snapshots, trees and objects are what they claim to be.
- **Working-tree files**: nothing velody reads from outside writes outside the
  repository's working tree.
- **Credentials**: bearer tokens and signing keys are never put in URLs, logs
  or bundles.
- **Availability of a hosted remote**: a `velody serve-http` listener keeps
  serving legitimate clients.

## Trust boundaries

- the **local repository** on disk (the user's own account is trusted);
- **bundles** received from anyone;
- the **ssh and HTTP wire** to a remote, in both directions;
- **git import** of a git repository's history;
- the **MCP server**, over stdio and over HTTP.

Everything that crosses one of these is untrusted bytes until validated.

## What Velody defends against

| Threat | Defence | Evidence |
|--------|---------|----------|
| Tampered history | Snapshot ids are recomputed on import and never taken from the sender; `velody fsck` re-verifies objects, ids and refs | [FORMAT.md](FORMAT.md) section 8, README "Integrity & safety" |
| Malicious bundles and remotes: path escape | One path rule (`safe_path`) applied at ingestion and before any working-tree write | [FORMAT.md](FORMAT.md) section 3.1 |
| Malicious bundles and remotes: resource exhaustion | Read limits on the handle (`Limits`), `LimitExceeded` instead of unbounded allocation | [FORMAT.md](FORMAT.md) section 9 |
| Malformed input crashing a reader | cargo-fuzz targets for bundles, wire, HTTP service, git import, paths and ids, run in CI | `fuzz/`, [.github/workflows/fuzz.yml](../.github/workflows/fuzz.yml) |
| Unauthenticated access to a hosted remote | `serve-http` bearer tokens, read-only mode, request limits; push is refused until a token is configured | [FORMAT.md](FORMAT.md) section 9.1, README "Hosting a remote" |
| Forged authorship | Ed25519 signatures over the snapshot id. The author metadata alone is not authentication; only a verified signature by a key you trust is | [FORMAT.md](FORMAT.md) section 11.1 |
| Interrupted operations (crash, kill, power loss) | Atomic writes and recoverable state, with every operation killed at every step in tests | [CRASH_SAFETY.md](CRASH_SAFETY.md) |

## What Velody does not defend against

- **A compromised host.** Anyone who can write the repository directory or run
  as the user can do anything the user can.
- **A trusted key that signs something wrong.** A signature proves who signed,
  not that the content is good.
- **Key management and revocation.** Distributing, rotating and revoking keys
  is left to the user.
- **Side channels** such as timing or size leaks.
- **Availability against a hostile peer on an unauthenticated listener**,
  beyond the configured limits. Put a listener you expose behind a token and a
  reverse proxy.
- **TLS.** `serve-http` speaks plain HTTP; terminating TLS is the proxy's job.
- **Git-level threats in exported histories.** Exporting to git carries the
  content across; git's own hooks, filters and hash weaknesses are git's.

## Residual risks

- The known crash windows, listed in [CRASH_SAFETY.md](CRASH_SAFETY.md).
- Later countersignatures are not re-sent: a countersignature added to an
  already-shared snapshot does not travel on a later sync
  ([FORMAT.md](FORMAT.md) section 9).
