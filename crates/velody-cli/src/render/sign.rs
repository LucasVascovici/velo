//! Render signing: key generation, `velody sign`, `velody verify`.

use console::style;
use std::path::Path;
use velody_core::sign::Verification;
use velody_core::SnapshotId;

/// The marker `history` and `show` put on a snapshot that carries a signature.
pub fn marker() -> String {
    style("[signed]").green().to_string()
}

/// The first 16 hex characters of a public key: enough to tell keys apart.
fn short_key(key: &str) -> &str {
    &key[..16.min(key.len())]
}

pub fn print_keygen(public_key: &str, out: &Path) {
    println!("{public_key}");
    eprintln!(
        "{} {} is a secret key; keep it private. Velody will never store or upload it.",
        style("note:").yellow().bold(),
        out.display()
    );
}

pub fn print_signed(snapshot: &SnapshotId, public_key: &str) {
    println!(
        "signed {} with {}",
        style(super::id::short(snapshot)).yellow(),
        short_key(public_key)
    );
}

pub fn print_verification(v: &Verification) {
    let id = style(super::id::short(&v.snapshot)).yellow();
    let groups: [(&str, &str, &Vec<String>); 4] = [
        ("trusted", "valid signature from a trusted key", &v.trusted),
        (
            "untrusted",
            "valid signature, but the key is not in your trust file",
            &v.untrusted,
        ),
        (
            "invalid",
            "signature does not match this snapshot",
            &v.invalid,
        ),
        (
            "unverifiable",
            "unknown algorithm or malformed key",
            &v.unverifiable,
        ),
    ];
    for (label, why, keys) in groups {
        for key in keys {
            println!("  {label:<12} {key}  ({why})");
        }
    }
    if v.is_trusted() {
        println!("snapshot {id} is signed by a trusted key.");
        return;
    }
    let reason = if !v.invalid.is_empty() {
        "it carries an invalid signature"
    } else if !v.untrusted.is_empty() || !v.unverifiable.is_empty() {
        "no signature on it comes from a trusted key"
    } else {
        "it is not signed"
    };
    println!("snapshot {id} is not trusted: {reason}.");
}
