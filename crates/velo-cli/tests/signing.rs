//! `velo keygen`, `velo sign`, `velo verify` and the `signed` marker.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stdout, stderr, exit code).
fn velo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run velo binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, err, code) = velo(dir, args);
    assert_eq!(code, 0, "velo {args:?} failed: {err}");
    out
}

/// A repository with two snapshots, plus a separate directory for key files.
fn setup() -> (TempDir, TempDir) {
    let repo = TempDir::new().unwrap();
    let keys = TempDir::new().unwrap();
    ok(repo.path(), &["init"]);
    for msg in ["first", "second"] {
        std::fs::write(repo.path().join("f.txt"), msg).unwrap();
        ok(repo.path(), &["save", msg]);
    }
    (repo, keys)
}

fn keygen(keys: &Path, name: &str) -> (String, String) {
    let path = keys.join(name);
    let public = ok(keys, &["keygen", "--out", path.to_str().unwrap()])
        .trim()
        .to_string();
    (path.to_str().unwrap().to_string(), public)
}

#[test]
fn keygen_writes_a_seed_prints_the_public_key_and_refuses_to_overwrite() {
    let keys = TempDir::new().unwrap();
    let path = keys.path().join("k");
    let (out, err, code) = velo(keys.path(), &["keygen", "--out", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim().len(), 64, "{out}");
    assert!(err.contains("secret"), "{err}");

    let seed = std::fs::read_to_string(&path).unwrap();
    assert!(seed.ends_with('\n'));
    assert_eq!(seed.trim().len(), 64);
    assert!(seed
        .trim()
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let (_, err, code) = velo(keys.path(), &["keygen", "--out", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
}

#[test]
fn sign_then_verify_with_the_key_trusted() {
    let (repo, keys) = setup();
    let (key, public) = keygen(keys.path(), "k");
    let trust = keys.path().join("t");
    std::fs::write(&trust, format!("# my key\n\n{public}\n")).unwrap();

    let out = ok(repo.path(), &["sign", "--key", &key]);
    assert!(out.starts_with("signed "), "{out}");
    assert!(out.contains(&format!("with {}", &public[..16])), "{out}");

    let out = ok(repo.path(), &["verify", "--trust", trust.to_str().unwrap()]);
    assert!(out.contains("trusted") && out.contains(&public), "{out}");
}

#[test]
fn verify_with_an_empty_trust_file_fails_and_lists_the_key_untrusted() {
    let (repo, keys) = setup();
    let (key, public) = keygen(keys.path(), "k");
    ok(repo.path(), &["sign", "--key", &key]);
    let trust = keys.path().join("t");
    std::fs::write(&trust, "").unwrap();

    let (out, _, code) = velo(repo.path(), &["verify", "--trust", trust.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("untrusted") && out.contains(&public), "{out}");
}

#[test]
fn verify_an_unsigned_snapshot_fails() {
    let (repo, keys) = setup();
    let (_, public) = keygen(keys.path(), "k");
    let trust = keys.path().join("t");
    std::fs::write(&trust, &public).unwrap();
    let (out, _, code) = velo(repo.path(), &["verify", "--trust", trust.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("not signed"), "{out}");
}

#[test]
fn a_different_seed_in_the_key_file_is_not_trusted_as_the_original() {
    let (repo, keys) = setup();
    let (key, original) = keygen(keys.path(), "k");
    let trust = keys.path().join("t");
    std::fs::write(&trust, &original).unwrap();
    // Replace the key file's seed: the signature is now by another key.
    std::fs::write(&key, format!("{}\n", "ab".repeat(32))).unwrap();

    ok(repo.path(), &["sign", "--key", &key]);
    let (out, _, code) = velo(repo.path(), &["verify", "--trust", trust.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("untrusted"), "{out}");
    assert!(!out.contains(&format!("trusted      {original}")), "{out}");
}

#[test]
fn history_and_show_mark_only_signed_snapshots() {
    let (repo, keys) = setup();
    let (key, _) = keygen(keys.path(), "k");
    // Sign the first snapshot only.
    let history = ok(repo.path(), &["history", "--oneline"]);
    let first_id = history
        .lines()
        .find(|l| l.contains("first"))
        .and_then(|l| {
            l.split_whitespace()
                .find(|w| w.len() == 16 && w.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        .expect("first snapshot line")
        .to_string();
    ok(repo.path(), &["sign", &first_id, "--key", &key]);

    for view in [
        &["history", "--oneline"][..],
        &["history"],
        &["history", "--graph"],
    ] {
        let out = ok(repo.path(), view);
        let signed: Vec<&str> = out.lines().filter(|l| l.contains("[signed]")).collect();
        assert_eq!(signed.len(), 1, "{view:?}: {out}");
        assert!(signed[0].contains("first"), "{view:?}: {out}");
    }
    assert!(ok(repo.path(), &["show", &first_id]).contains("[signed]"));
    assert!(!ok(repo.path(), &["show", "main"]).contains("[signed]"));
}
