//! `velo report`, driven through the binary against a repository built with
//! velo-core, including hostile text in every field.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use velo_core::sign::{self, SigningKey};
use velo_core::tree::{SaveTree, TreeEntry};
use velo_core::{Author, BranchName, Repo, SnapshotMeta};

const HOSTILE: &str = "<script>alert(1)</script>";

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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

const DAY: i64 = 86_400_000;

/// main: an old clean snapshot (10 days ago) and a recent hostile one signed
/// by key 7; side: one snapshot. Returns the repo and the trust file path.
fn setup() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert_eq!(velo(d, &["init"]).2, 0);
    let repo = Repo::open_and_migrate(d).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let side: BranchName = "side".parse().unwrap();
    let now = now_ms();

    let first = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main,
            parent: None,
            merge_parent: None,
            message: "old and clean",
            entries: vec![TreeEntry::file("a.txt", b"one\n".to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(now - 10 * DAY),
            renames: &[],
        })
        .unwrap();

    let author = Author::with_email(HOSTILE, "a@b.c").unwrap();
    let mut meta = SnapshotMeta::new();
    meta.set(
        HOSTILE,
        "key\u{202E}",
        "v\u{1b}[31m<img src=x onerror=alert(1)>",
    )
    .unwrap();
    let hostile_text = format!("one\n{HOSTILE}\n");
    let second = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main,
            parent: Some(&first),
            merge_parent: None,
            message: &format!("{HOSTILE} \u{202E}evil\nsecond line"),
            entries: vec![
                TreeEntry::file("a.txt", hostile_text.into_bytes()),
                TreeEntry::file("<b>.txt", b"x\n".to_vec()),
            ],
            meta,
            author: Some(&author),
            timestamp_ms: Some(now - 1000),
            renames: &[],
        })
        .unwrap();
    let key = SigningKey::from_seed([7; 32]);
    let sig = sign::sign(&repo.write().unwrap(), &second, &key).unwrap();

    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &side,
            parent: None,
            merge_parent: None,
            message: "on the side",
            entries: vec![TreeEntry::file("s.txt", b"side\n".to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(now - 2000),
            renames: &[],
        })
        .unwrap();

    let trust = d.join("trusted.keys");
    std::fs::write(&trust, format!("# trusted\n{}\n", sig.public_key)).unwrap();
    (tmp, trust)
}

fn html_of(d: &Path, extra: &[&str]) -> String {
    let mut args = vec!["report", "-o", "out.html"];
    args.extend_from_slice(extra);
    let (out, err, code) = velo(d, &args);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("out.html"), "{out}");
    std::fs::read_to_string(d.join("out.html")).unwrap()
}

fn json_of(d: &Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["report", "--json"];
    args.extend_from_slice(extra);
    let (out, err, code) = velo(d, &args);
    assert_eq!(code, 0, "{err}");
    serde_json::from_str(&out).expect("valid JSON on stdout")
}

#[test]
fn html_is_self_contained_and_escapes_hostile_text() {
    let (tmp, _) = setup();
    let html = html_of(tmp.path(), &[]);
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("Content-Security-Policy"));
    assert!(html.contains("default-src 'none'"));
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<script"));
    for bad in ["http:", "https:", "//", "href=", "src="] {
        // The escaped hostile text legitimately contains `src=x`.
        if bad == "src=" {
            continue;
        }
        assert!(!lower.contains(bad), "{bad} in report");
    }
    // The hostile text is there, as text.
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("<img"));
    assert!(html.contains("&lt;b&gt;.txt"));
    // Bidi and control characters are visible, not raw.
    assert!(html.contains("\\u{202E}"));
    assert!(html.contains("\\u{001B}"));
    assert!(!html.contains('\u{202E}'));
    assert!(!html.contains('\u{1b}'));
    // Verdict, both branches, signature badge, provenance.
    assert!(html.contains("snapshot ids: OK"), "{html}");
    assert!(html.contains("side") && html.contains("on the side"));
    assert!(html.contains("badge untrusted"));
    assert!(html.contains("badge unsigned"));
    assert!(html.contains("Line provenance"));
}

#[test]
fn trust_file_turns_the_badge_trusted() {
    let (tmp, trust) = setup();
    let html = html_of(tmp.path(), &["--trust", trust.to_str().unwrap()]);
    assert!(html.contains("badge trusted"));
    assert!(!html.contains("badge untrusted"));
}

#[test]
fn no_fsck_says_not_checked() {
    let (tmp, _) = setup();
    let html = html_of(tmp.path(), &["--no-fsck"]);
    assert!(html.contains("not checked"));
}

#[test]
fn json_has_every_snapshot_signature_path_and_line() {
    let (tmp, trust) = setup();
    let v = json_of(tmp.path(), &["--trust", trust.to_str().unwrap()]);
    let branches = v["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    let main = branches.iter().find(|b| b["branch"] == "main").unwrap();
    let snaps = main["snapshots"].as_array().unwrap();
    assert_eq!(snaps.len(), 2);
    let newest = &snaps[0];
    assert!(newest["message"].as_str().unwrap().contains(HOSTILE));
    assert_eq!(newest["author"]["name"], HOSTILE);
    assert_eq!(newest["signatures"][0]["state"], "trusted");
    assert!(newest["meta"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["namespace"] == HOSTILE));
    let paths: Vec<&str> = newest["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"a.txt") && paths.contains(&"<b>.txt"));
    assert!(snaps[1]["signatures"].as_array().unwrap().is_empty());
    let files = v["files"].as_array().unwrap();
    let a = files.iter().find(|f| f["path"] == "a.txt").unwrap();
    let lines = a["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["text"], HOSTILE);
    assert!(lines[1]["snapshot"].is_string());
    assert_eq!(v["integrity"]["healthy"], true);
}

#[test]
fn branch_filter_limits_the_report() {
    let (tmp, _) = setup();
    let v = json_of(tmp.path(), &["--branch", "side"]);
    let branches = v["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0]["branch"], "side");
}

#[test]
fn since_accepts_date_rfc3339_and_relative() {
    let (tmp, _) = setup();
    let d = tmp.path();
    let count = |v: &serde_json::Value| -> usize {
        v["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["branch"] == "main")
            .unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(count(&json_of(d, &["--since", "2000-01-01"])), 2);
    assert_eq!(count(&json_of(d, &["--since", "2000-01-01T00:00:00Z"])), 2);
    assert_eq!(count(&json_of(d, &["--since", "30d"])), 2);
    assert_eq!(count(&json_of(d, &["--since", "5d"])), 1);
    assert_eq!(count(&json_of(d, &["--since", "24h"])), 1);
    assert_eq!(count(&json_of(d, &["--since", "1w"])), 1);
    assert_eq!(count(&json_of(d, &["--since", "2999-01-01"])), 0);
}

#[test]
fn bad_since_is_a_usage_error() {
    let (tmp, _) = setup();
    for bad in ["yesterday", "7", "7x", "2024-13-45"] {
        let (_, err, code) = velo(tmp.path(), &["report", "--since", bad]);
        assert_eq!(code, 2, "{bad}: {err}");
        assert!(err.contains("--since"), "{err}");
    }
}

#[test]
fn json_to_a_file_writes_it() {
    let (tmp, _) = setup();
    let (out, err, code) = velo(tmp.path(), &["report", "--json", "-o", "r.json"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("r.json"));
    let text = std::fs::read_to_string(tmp.path().join("r.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap();
}

#[test]
fn default_output_is_velo_report_html() {
    let (tmp, _) = setup();
    let (_, err, code) = velo(tmp.path(), &["report"]);
    assert_eq!(code, 0, "{err}");
    assert!(tmp.path().join("velo-report.html").exists());
}
