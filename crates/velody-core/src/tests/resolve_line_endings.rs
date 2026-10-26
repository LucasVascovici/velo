//! `resolve` writes a consistently CRLF file back with CRLF endings.

use std::fs;
use tempfile::TempDir;

use crate::commands::resolve::{self, uses_crlf, OnEdits, TakeOption};
use crate::commands::{self, merge, save, status, switch};
use crate::Repo;

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

/// Merge `feat` into `main` over `F.md`, then take both sides; returns the bytes.
fn resolved_bytes(base: &[u8], ours: &[u8], theirs: &[u8]) -> (TempDir, Repo, Vec<u8>) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let f = tmp.path().join("F.md");
    fs::write(&f, base).unwrap();
    save_as(&repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "feat", false).unwrap();
    }
    fs::write(&f, theirs).unwrap();
    save_as(&repo, "feat edit");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", true).unwrap();
    }
    fs::write(&f, ours).unwrap();
    save_as(&repo, "main edit");
    {
        let g = repo.write().unwrap();
        merge::run(&g, merge::Mode::Bring { source: "feat" }).unwrap();
    }
    let row = resolve::get_conflict(&repo, "F.md").unwrap();
    {
        let g = repo.write().unwrap();
        resolve::take_side(&g, &row, TakeOption::Both, OnEdits::Refuse).unwrap();
    }
    let bytes = fs::read(&f).unwrap();
    (tmp, repo, bytes)
}

#[test]
fn crlf_file_is_resolved_with_crlf() {
    let (_t, repo, b) =
        resolved_bytes(b"a\r\nb\r\nc\r\n", b"a\r\nB1\r\nc\r\n", b"a\r\nB2\r\nc\r\n");
    let s = String::from_utf8(b).unwrap();
    assert!(s.contains("B1") && s.contains("B2"));
    assert_eq!(s.matches('\n').count(), s.matches("\r\n").count());
    save_as(&repo, "resolved");
    assert!(status::run(&repo, &[]).unwrap().is_clean());
}

#[test]
fn lf_file_is_resolved_with_lf() {
    let (_t, _r, b) = resolved_bytes(b"a\nb\nc\n", b"a\nB1\nc\n", b"a\nB2\nc\n");
    assert!(!b.contains(&b'\r'));
}

#[test]
fn mixed_file_is_resolved_with_lf() {
    let (_t, _r, b) = resolved_bytes(b"a\r\nb\nc\r\n", b"a\r\nB1\nc\r\n", b"a\r\nB2\nc\r\n");
    assert!(!b.contains(&b'\r'));
}

#[test]
fn uses_crlf_classifies_bytes() {
    assert!(uses_crlf(b"a\r\nb\r\n"));
    assert!(!uses_crlf(b"a\nb\n"));
    assert!(!uses_crlf(b"a\r\nb\n"));
    assert!(!uses_crlf(b"no newline"));
    assert!(!uses_crlf(b"a\r\n\0"));
    assert!(!uses_crlf(b""));
}
