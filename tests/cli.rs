//! End-to-end CLI tests: drive the real `velo` binary and assert on its actual
//! stdout. These complement the in-process unit tests (which mostly guard
//! against panics for the display commands) by verifying the *output* users see.
//!
//! Cargo builds the binary and exposes its path via `CARGO_BIN_EXE_velo`.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stdout, success).
fn velo(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velo binary");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

/// A repo with a couple of snapshots to exercise the read-only commands.
fn repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velo(d, &["init"]).1);
    write(d, "app.py", "def main():\n    print(\"hello\")\n    return 0\n");
    assert!(velo(d, &["save", "initial commit"]).1);
    write(d, "app.py", "def main():\n    print(\"hello world\")\n    return 0\n");
    assert!(velo(d, &["save", "tweak greeting"]).1);
    tmp
}

#[test]
fn status_reports_clean_and_dirty() {
    let tmp = repo();
    let d = tmp.path();
    let (out, ok) = velo(d, &["status"]);
    assert!(ok);
    assert!(out.contains("clean"), "clean tree should say so:\n{out}");

    write(d, "app.py", "changed\n");
    let (out, ok) = velo(d, &["status"]);
    assert!(ok);
    assert!(out.contains("app.py") && out.contains("Modified"), "should list the change:\n{out}");
}

#[test]
fn diff_shows_the_actual_change() {
    let tmp = repo();
    let d = tmp.path();
    write(d, "app.py", "def main():\n    print(\"goodbye\")\n    return 0\n");
    let (out, ok) = velo(d, &["diff"]);
    assert!(ok);
    assert!(out.contains("goodbye"), "diff must show the new line:\n{out}");
    assert!(out.contains("hello world"), "diff must show the removed line:\n{out}");
}

#[test]
fn grep_finds_matches_and_respects_flags() {
    let tmp = repo();
    let d = tmp.path();
    let (out, ok) = velo(d, &["grep", "print"]);
    assert!(ok);
    assert!(out.contains("app.py") && out.contains("print"), "grep should find the match:\n{out}");

    // -l prints only file names (no code line content like the paren).
    let (out, ok) = velo(d, &["grep", "-l", "print"]);
    assert!(ok);
    assert!(out.contains("app.py"), "grep -l lists the file:\n{out}");

    // case-insensitive
    let (out, ok) = velo(d, &["grep", "-i", "PRINT"]);
    assert!(ok);
    assert!(out.contains("print"), "grep -i should match case-insensitively:\n{out}");

    // no match → still succeeds, no file listed
    let (out, ok) = velo(d, &["grep", "zzz_nomatch_zzz"]);
    assert!(ok);
    assert!(!out.contains("app.py"), "no-match grep should not list files:\n{out}");
}

#[test]
fn history_and_graph_show_commits() {
    let tmp = repo();
    let d = tmp.path();
    let (out, ok) = velo(d, &["history", "--oneline"]);
    assert!(ok);
    assert!(out.contains("initial commit") && out.contains("tweak greeting"), "history lists messages:\n{out}");

    let (out, ok) = velo(d, &["history", "--graph"]);
    assert!(ok);
    assert!(out.contains("tweak greeting"), "graph shows commits:\n{out}");
}

#[test]
fn show_and_blame_render() {
    let tmp = repo();
    let d = tmp.path();
    // resolve the latest hash from history --oneline
    let (hist, ok) = velo(d, &["history", "--oneline"]);
    assert!(ok);
    let hash = hist
        .lines()
        .find_map(|l| l.split_whitespace().find(|w| w.len() >= 12 && w.chars().all(|c| c.is_ascii_hexdigit())))
        .expect("a hash in history output")
        .to_string();

    let (out, ok) = velo(d, &["show", &hash]);
    assert!(ok);
    assert!(out.contains("tweak greeting"), "show renders the snapshot:\n{out}");

    let (out, ok) = velo(d, &["blame", "app.py"]);
    assert!(ok);
    assert!(out.contains("hello world"), "blame shows annotated lines:\n{out}");
}

#[test]
fn fsck_reports_health() {
    let tmp = repo();
    let d = tmp.path();
    let (out, ok) = velo(d, &["fsck"]);
    assert!(ok, "fsck should succeed on a healthy repo:\n{out}");
    assert!(out.contains("healthy"), "fsck should report health:\n{out}");
}

#[test]
fn bundle_transfers_history_between_repos() {
    // Repo A with two branches + a tag.
    let a_tmp = TempDir::new().unwrap();
    let a = a_tmp.path();
    assert!(velo(a, &["init"]).1);
    write(a, "app.py", "print('a')\n");
    assert!(velo(a, &["save", "initial"]).1);
    assert!(velo(a, &["switch", "feature"]).1);
    write(a, "feat.py", "print('feature')\n");
    assert!(velo(a, &["save", "feature work"]).1);
    assert!(velo(a, &["switch", "main"]).1);
    assert!(velo(a, &["tag", "v1"]).1);

    // Bundle the whole repo to a file outside both repos.
    let share = TempDir::new().unwrap();
    let bundle = share.path().join("proj.velo");
    let bundle = bundle.to_str().unwrap();
    let (out, ok) = velo(a, &["bundle", "create", bundle]);
    assert!(ok, "bundle create failed:\n{out}");

    // Repo B imports it.
    let b_tmp = TempDir::new().unwrap();
    let b = b_tmp.path();
    assert!(velo(b, &["init"]).1);
    let (out, ok) = velo(b, &["bundle", "apply", bundle]);
    assert!(ok, "bundle apply failed:\n{out}");

    // B has A's history, branches, and tag.
    let (hist, ok) = velo(b, &["history", "--all", "--oneline"]);
    assert!(ok);
    assert!(hist.contains("initial") && hist.contains("feature work"), "B is missing history:\n{hist}");
    let (tags, _) = velo(b, &["tag"]);
    assert!(tags.contains("v1"), "tag not transferred:\n{tags}");

    // The imported feature content restores byte-for-byte.
    assert!(velo(b, &["switch", "feature", "--force"]).1);
    assert_eq!(std::fs::read_to_string(b.join("feat.py")).unwrap(), "print('feature')\n");

    // Receiver is consistent, and re-apply is a no-op.
    assert!(velo(b, &["fsck"]).1, "B must pass fsck after import");
    let (out, ok) = velo(b, &["bundle", "apply", bundle]);
    assert!(ok && out.contains("up to date"), "re-apply should be idempotent:\n{out}");
}

#[test]
fn clone_push_pull_full_collaboration_loop() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    assert!(velo(&origin, &["init"]).1);
    write(&origin, "shared.txt", "base\n");
    assert!(velo(&origin, &["save", "C0"]).1);
    let origin_s = origin.to_str().unwrap();

    // Two developers clone.
    assert!(velo(root.path(), &["clone", origin_s, "A"]).1);
    assert!(velo(root.path(), &["clone", origin_s, "B"]).1);
    let a = root.path().join("A");
    let b = root.path().join("B");

    // A edits and pushes (fast-forward).
    write(&a, "a.txt", "a-work\n");
    assert!(velo(&a, &["save", "A1"]).1);
    let (out, ok) = velo(&a, &["push"]);
    assert!(ok && out.contains("Pushed"), "A push (ff) should succeed:\n{out}");

    // B commits, then its push is rejected (origin moved under it).
    write(&b, "b.txt", "b-work\n");
    assert!(velo(&b, &["save", "B1"]).1);
    let (out, ok) = velo(&b, &["push"]);
    assert!(!ok && out.to_lowercase().contains("rejected"), "B push must be non-ff rejected:\n{out}");

    // B pulls → divergence is reported (not auto-merged).
    let (out, ok) = velo(&b, &["pull"]);
    assert!(ok && out.to_lowercase().contains("diverged"), "B pull should report divergence:\n{out}");

    // B reconciles via the existing merge engine, then pushes (now ff).
    let (out, ok) = velo(&b, &["merge", "origin/main"]);
    assert!(ok, "merge origin/main should succeed:\n{out}");
    assert!(velo(&b, &["save", "Merge origin/main"]).1);
    assert!(b.join("a.txt").exists() && b.join("b.txt").exists(), "B must have both sides' files");
    let (out, ok) = velo(&b, &["push"]);
    assert!(ok && out.contains("Pushed"), "B push after merge should succeed:\n{out}");

    // A pulls → fast-forwards and gains B's file.
    let (out, ok) = velo(&a, &["pull"]);
    assert!(ok && out.to_lowercase().contains("fast-forward"), "A pull should fast-forward:\n{out}");
    assert!(a.join("b.txt").exists(), "A should now have b.txt");

    // Everyone converges on the same content-addressed tip, and both verify.
    let a_tip = std::fs::read_to_string(a.join(".velo/PARENT")).unwrap();
    let b_tip = std::fs::read_to_string(b.join(".velo/PARENT")).unwrap();
    assert_eq!(a_tip, b_tip, "A and B must converge on the same tip");
    assert!(velo(&a, &["fsck"]).1, "A must pass fsck");
    assert!(velo(&b, &["fsck"]).1, "B must pass fsck");
}

#[test]
fn sync_over_streaming_child_protocol() {
    // Exercises the full client↔server pack protocol (transport::StreamRemote +
    // serve-upload/serve-receive) via the `child:` scheme, which runs the server
    // as a local subprocess — the same path ssh uses, minus the network.
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    assert!(velo(&origin, &["init"]).1);
    write(&origin, "shared.txt", "base\n");
    assert!(velo(&origin, &["save", "C0"]).1);
    let origin_url = format!("child:{}", origin.to_str().unwrap());

    // Clone over the streaming protocol.
    let (out, ok) = velo(root.path(), &["clone", &origin_url, "A"]);
    assert!(ok, "streaming clone failed:\n{out}");
    let (out, ok) = velo(root.path(), &["clone", &origin_url, "B"]);
    assert!(ok, "streaming clone B failed:\n{out}");
    let a = root.path().join("A");
    let b = root.path().join("B");

    // A pushes over the protocol (fast-forward).
    write(&a, "a.txt", "a\n");
    assert!(velo(&a, &["save", "A1"]).1);
    let (out, ok) = velo(&a, &["push"]);
    assert!(ok && out.contains("Pushed"), "streaming push failed:\n{out}");

    // B diverges → its push is rejected by the server.
    write(&b, "b.txt", "b\n");
    assert!(velo(&b, &["save", "B1"]).1);
    let (out, ok) = velo(&b, &["push"]);
    assert!(!ok && out.to_lowercase().contains("rejected"), "streaming non-ff must be rejected:\n{out}");

    // B pulls (diverged), merges, and pushes.
    let (out, ok) = velo(&b, &["pull"]);
    assert!(ok && out.to_lowercase().contains("diverged"), "streaming pull should report divergence:\n{out}");
    assert!(velo(&b, &["merge", "origin/main"]).1);
    assert!(velo(&b, &["save", "merge"]).1);
    let (out, ok) = velo(&b, &["push"]);
    assert!(ok && out.contains("Pushed"), "streaming push after merge failed:\n{out}");

    // A pulls → fast-forward and gains b.txt.
    let (out, ok) = velo(&a, &["pull"]);
    assert!(ok && out.to_lowercase().contains("fast-forward"), "streaming pull ff failed:\n{out}");
    assert!(a.join("b.txt").exists() && a.join("a.txt").exists());

    assert!(velo(&a, &["fsck"]).1);
    assert!(velo(&b, &["fsck"]).1);
}

#[test]
fn status_reports_ahead_behind_and_diverged() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    assert!(velo(&origin, &["init"]).1);
    write(&origin, "f.txt", "base\n");
    assert!(velo(&origin, &["save", "C0"]).1);
    let origin_s = origin.to_str().unwrap();

    assert!(velo(root.path(), &["clone", origin_s, "A"]).1);
    assert!(velo(root.path(), &["clone", origin_s, "B"]).1);
    let a = root.path().join("A");
    let b = root.path().join("B");

    // Fresh clone → in sync.
    let (out, _) = velo(&a, &["status"]);
    assert!(out.contains("up to date with origin/main"), "fresh clone should be in sync:\n{out}");

    // A commits → ahead.
    write(&a, "a.txt", "a\n");
    assert!(velo(&a, &["save", "A1"]).1);
    let (out, _) = velo(&a, &["status"]);
    assert!(out.contains("1 ahead of origin/main"), "should report ahead:\n{out}");

    // A pushes; B fetches → behind.
    assert!(velo(&a, &["push"]).1);
    assert!(velo(&b, &["fetch"]).1);
    let (out, _) = velo(&b, &["status"]);
    assert!(out.contains("1 behind origin/main"), "should report behind:\n{out}");

    // B also commits → diverged.
    write(&b, "b.txt", "b\n");
    assert!(velo(&b, &["save", "B1"]).1);
    let (out, _) = velo(&b, &["status"]);
    assert!(out.contains("diverged from origin/main"), "should report divergence:\n{out}");
    assert!(out.contains("1 ahead, 1 behind"), "should show both counts:\n{out}");
}

#[test]
fn fetch_is_readonly_and_tracks_remote() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    assert!(velo(&origin, &["init"]).1);
    write(&origin, "f.txt", "one\n");
    assert!(velo(&origin, &["save", "first"]).1);
    let origin_s = origin.to_str().unwrap();

    assert!(velo(root.path(), &["clone", origin_s, "C"]).1);
    let c = root.path().join("C");

    // Origin advances on its own.
    write(&origin, "f.txt", "two\n");
    assert!(velo(&origin, &["save", "second"]).1);

    // Fetch pulls the history but must not touch C's working tree.
    let (out, ok) = velo(&c, &["fetch"]);
    assert!(ok, "fetch failed:\n{out}");
    assert_eq!(std::fs::read_to_string(c.join("f.txt")).unwrap(), "one\n", "fetch must not change the working tree");

    // The remote-tracking ref resolves to origin's new commit.
    let (out, ok) = velo(&c, &["show", "origin/main"]);
    assert!(ok && out.contains("second"), "origin/main should resolve to the fetched commit:\n{out}");
    assert!(velo(&c, &["fsck"]).1);
}

#[test]
fn fetch_then_pull_still_fast_forwards() {
    // Regression: `fetch` parks incoming commits on the remote-tracking branch.
    // A following `pull` used to (a) report divergence, because its ancestry walk
    // only consulted the (now-empty) pack, and (b) not advance the local branch,
    // because Velo derives branch tips from the `branch` column and nothing
    // re-labelled the fetched commits onto it.
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    assert!(velo(&origin, &["init"]).1);
    write(&origin, "f.txt", "one\n");
    assert!(velo(&origin, &["save", "first"]).1);
    let origin_s = origin.to_str().unwrap();

    assert!(velo(root.path(), &["clone", origin_s, "C"]).1);
    let c = root.path().join("C");

    // Origin advances.
    write(&origin, "g.txt", "two\n");
    assert!(velo(&origin, &["save", "second"]).1);

    // Fetch first (read-only), then pull.
    assert!(velo(&c, &["fetch"]).1);
    let (out, ok) = velo(&c, &["pull"]);
    assert!(ok, "pull after fetch failed:\n{out}");
    assert!(
        out.to_lowercase().contains("fast-forward"),
        "pull after fetch must fast-forward, not report divergence:\n{out}"
    );
    assert!(c.join("g.txt").exists(), "pull must bring the new file into the tree");

    // The local branch really advanced: status is up to date, not behind.
    let (out, _) = velo(&c, &["status"]);
    assert!(
        out.contains("up to date with origin/main"),
        "local branch must have advanced after the fast-forward:\n{out}"
    );
    assert!(velo(&c, &["fsck"]).1);
}

#[test]
fn fsck_fails_and_repairs_are_exit_coded() {
    let tmp = repo();
    let d = tmp.path();
    // Corrupt an object → fsck must exit non-zero.
    let objects = d.join(".velo/objects");
    let first = std::fs::read_dir(&objects)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_file())
        .unwrap();
    std::fs::write(&first, b"corrupt").unwrap();
    let (out, ok) = velo(d, &["fsck"]);
    assert!(!ok, "fsck must fail on corruption:\n{out}");
}
