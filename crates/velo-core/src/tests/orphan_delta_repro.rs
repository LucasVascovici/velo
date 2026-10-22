//! Regression for the undo-heavy differential finding (seed 1000002 on the
//! `format-v4` golden): `Redo, Undo, Edit, Save` left a delta row whose base
//! was gone. 20.6 promises that no removal leaves such a row.

use std::path::Path;

use crate::commands::{fsck, redo, save, undo};
use crate::Repo;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn redo_undo_edit_save_leaves_no_orphan_delta() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("a");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/repos/format-v4");
    copy_dir(&src, &root);
    let mut repo = Repo::open_and_migrate(&root).unwrap();
    repo.set_keyframe_interval(1);

    let clock = |step: i64| repo.test_now_ms.set(Some(1_700_000_000_000 + step * 1_000));
    clock(0);
    let _ = redo::run(&repo.write().unwrap());
    clock(1);
    let _ = undo::run(&repo.write().unwrap());
    // Edit { file: 1, variant: Some(2) }: the body of the differential harness.
    let hot = 2usize;
    let body: String = (0..5)
        .map(|j| format!("file 1 line {j} v{}\n", if j == hot { 2 } else { 0 }))
        .collect();
    clock(2);
    std::fs::create_dir_all(root.join("d")).unwrap();
    std::fs::write(root.join("d/f1.txt"), body).unwrap();
    clock(3);
    let _ = save::run(&repo.write().unwrap(), Some("step 3"), Default::default());

    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(
        crate::tree_store::orphan_delta_snapshots(repo.conn()).unwrap(),
        0
    );
    assert!(report.cruft.is_empty(), "{:?}", report.cruft);
}
