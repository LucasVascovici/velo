//! Signed snapshots (docs/FORMAT.md 11.1).
//!
//! A signature is an Ed25519 signature over a snapshot id. It is stored beside
//! the snapshot, never inside it, so signing does not change an id, a snapshot
//! can carry several signatures, and a countersignature never mints a new
//! snapshot.
//!
//! Velo verifies signatures; which keys to trust is the caller's policy, given
//! as a [`TrustPolicy`]. Velo stores no private keys: a [`SigningKey`] is taken
//! per call and never written. Key generation is deliberately absent, so the
//! core carries no random-number generator (and still builds for wasm).

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signer, VerifyingKey};

use crate::error::RefKind;
use crate::{Error, Repo, Result, SnapshotId, WriteGuard};

/// The algorithm name stored for Ed25519 (RFC 8032, pure, no prehash).
pub const ED25519: &str = "ed25519";

const PREFIX: &[u8] = b"velo-signature-v1\n";

/// The exact bytes that are signed for `snapshot`: the ASCII line
/// `velo-signature-v1` followed by the 64-character lowercase hex id.
pub fn message(snapshot: &SnapshotId) -> Vec<u8> {
    message_for(snapshot.as_str())
}

fn message_for(snapshot_hex: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREFIX.len() + snapshot_hex.len());
    out.extend_from_slice(PREFIX);
    out.extend_from_slice(snapshot_hex.as_bytes());
    out
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0xf) as usize] as char);
    }
    out
}

/// Decode exactly `N` bytes from lowercase or uppercase hex.
fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let raw = text.as_bytes();
    if raw.len() != N * 2 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, pair) in raw.chunks(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

/// A private signing key. Never stored by velo; never printed.
#[derive(Clone)]
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    /// Build a key from its 32-byte seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        SigningKey(ed25519_dalek::SigningKey::from_bytes(&seed))
    }

    /// Build a key from its seed written as 64 hex characters. Anything else
    /// is [`Error::InvalidInput`].
    pub fn from_hex(text: &str) -> Result<Self> {
        from_hex::<32>(text.trim())
            .map(Self::from_seed)
            .ok_or_else(|| Error::invalid("a signing key is 64 hex characters."))
    }

    /// The public half.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }
}

impl fmt::Debug for SigningKey {
    /// Prints the public key only, so a key cannot leak through `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey(public: {})", self.public_key())
    }
}

/// A 32-byte Ed25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// The raw key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_hex(&self.0))
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        from_hex::<32>(text)
            .map(PublicKey)
            .ok_or_else(|| Error::invalid(format!("'{text}' is not a 64-hex public key.")))
    }
}

/// One stored signature over a snapshot id. Keys and signatures are hex.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Signature {
    pub snapshot: SnapshotId,
    pub algorithm: String,
    pub public_key: String,
    pub signature: String,
}

/// Which keys the caller trusts. Velo never owns this decision.
pub trait TrustPolicy {
    /// Whether a (cryptographically valid) signature by `public_key` under
    /// `algorithm` should count as trusted. Called with the key as stored hex.
    fn trusts(&self, algorithm: &str, public_key: &str) -> bool;
}

/// A set of trusted Ed25519 public keys: the common [`TrustPolicy`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedKeys(HashSet<PublicKey>);

impl TrustedKeys {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: PublicKey) {
        self.0.insert(key);
    }
}

impl FromIterator<PublicKey> for TrustedKeys {
    fn from_iter<I: IntoIterator<Item = PublicKey>>(iter: I) -> Self {
        TrustedKeys(iter.into_iter().collect())
    }
}

impl TrustPolicy for TrustedKeys {
    fn trusts(&self, algorithm: &str, public_key: &str) -> bool {
        algorithm == ED25519
            && from_hex::<32>(public_key).is_some_and(|k| self.0.contains(&PublicKey(k)))
    }
}

/// The result of checking every signature on one snapshot. Keys are hex.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Verification {
    pub snapshot: SnapshotId,
    /// Valid, and the policy trusts the key.
    pub trusted: Vec<String>,
    /// Valid, but the policy does not trust the key.
    pub untrusted: Vec<String>,
    /// The signature does not verify.
    pub invalid: Vec<String>,
    /// An algorithm this build does not know: neither valid nor invalid.
    pub unverifiable: Vec<String>,
}

impl Verification {
    /// At least one trusted signature and no invalid one.
    pub fn is_trusted(&self) -> bool {
        !self.trusted.is_empty() && self.invalid.is_empty()
    }
}

/// How one stored row fared against its own claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowCheck {
    Valid,
    Invalid,
    /// An algorithm this build does not know; never treated as valid.
    Unverifiable,
}

/// Check one stored row. Malformed hex under a known algorithm is `Invalid`.
pub(crate) fn check_row(
    snapshot_hex: &str,
    algorithm: &str,
    public_key_hex: &str,
    signature_hex: &str,
) -> RowCheck {
    if algorithm != ED25519 {
        return RowCheck::Unverifiable;
    }
    let (Some(key), Some(sig)) = (
        from_hex::<32>(public_key_hex),
        from_hex::<64>(signature_hex),
    ) else {
        return RowCheck::Invalid;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return RowCheck::Invalid;
    };
    let sig = ed25519_dalek::Signature::from_bytes(&sig);
    match key.verify_strict(&message_for(snapshot_hex), &sig) {
        Ok(()) => RowCheck::Valid,
        Err(_) => RowCheck::Invalid,
    }
}

pub(crate) fn table_exists(conn: &rusqlite::Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'signatures')",
        [],
        |r| r.get::<_, bool>(0),
    )?)
}

/// Sign `snapshot` with `key` and store the signature.
///
/// The snapshot must be a live row. Ed25519 is deterministic, so signing again
/// with the same key yields the same row and stores nothing new.
pub fn sign(guard: &WriteGuard, snapshot: &SnapshotId, key: &SigningKey) -> Result<Signature> {
    let conn = guard.conn();
    let live: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM snapshots WHERE hash = ?)",
        [snapshot],
        |r| r.get(0),
    )?;
    if !live {
        return Err(Error::NotFound {
            kind: RefKind::Snapshot,
            name: snapshot.as_str().to_string(),
        });
    }
    let sig = key.0.sign(&message(snapshot));
    let row = Signature {
        snapshot: snapshot.clone(),
        algorithm: ED25519.to_string(),
        public_key: key.public_key().to_string(),
        signature: to_hex(&sig.to_bytes()),
    };
    // A repository opened without migration may predate the table.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS signatures (
            snapshot_hash TEXT NOT NULL,
            algorithm     TEXT NOT NULL,
            public_key    TEXT NOT NULL,
            signature     TEXT NOT NULL,
            PRIMARY KEY (snapshot_hash, public_key)
        );",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO signatures (snapshot_hash, algorithm, public_key, signature)
         VALUES (?, ?, ?, ?)",
        rusqlite::params![snapshot, row.algorithm, row.public_key, row.signature],
    )?;
    Ok(row)
}

/// Every signature stored for `snapshot`, in key order. A repository without
/// the table has none.
pub fn signatures(repo: &Repo, snapshot: &SnapshotId) -> Result<Vec<Signature>> {
    let conn = repo.conn();
    if !table_exists(conn)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT algorithm, public_key, signature FROM signatures
         WHERE snapshot_hash = ? ORDER BY public_key",
    )?;
    let rows = stmt.query_map([snapshot], |r| {
        Ok(Signature {
            snapshot: snapshot.clone(),
            algorithm: r.get(0)?,
            public_key: r.get(1)?,
            signature: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Check every signature on `snapshot` and sort the keys by outcome. The
/// `policy` decides which valid signatures are trusted.
pub fn verify(
    repo: &Repo,
    snapshot: &SnapshotId,
    policy: &dyn TrustPolicy,
) -> Result<Verification> {
    let mut out = Verification {
        snapshot: snapshot.clone(),
        trusted: Vec::new(),
        untrusted: Vec::new(),
        invalid: Vec::new(),
        unverifiable: Vec::new(),
    };
    for row in signatures(repo, snapshot)? {
        let bucket = match check_row(
            snapshot.as_str(),
            &row.algorithm,
            &row.public_key,
            &row.signature,
        ) {
            RowCheck::Valid if policy.trusts(&row.algorithm, &row.public_key) => &mut out.trusted,
            RowCheck::Valid => &mut out.untrusted,
            RowCheck::Invalid => &mut out.invalid,
            RowCheck::Unverifiable => &mut out.unverifiable,
        };
        bucket.push(row.public_key);
    }
    Ok(out)
}

/// The snapshots that carry at least one signature row, whether or not it
/// verifies. For markers and for protecting signed history.
pub fn signed_snapshots(repo: &Repo) -> Result<HashSet<SnapshotId>> {
    let conn = repo.conn();
    if !table_exists(conn)? {
        return Ok(HashSet::new());
    }
    let mut stmt = conn.prepare("SELECT DISTINCT snapshot_hash FROM signatures")?;
    let hashes = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    Ok(hashes.into_iter().map(SnapshotId::from_stored).collect())
}
