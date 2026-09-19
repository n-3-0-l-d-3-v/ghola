//! Property tests (ticket 006): moving a real directory from one snapshot to
//! another lands exactly on the target, for arbitrary pairs of snapshots.

use proptest::prelude::*;
use repo::Files;
use worktree::{apply, read_files};

fn arb_path() -> impl Strategy<Value = Vec<u8>> {
    (prop::collection::vec(0u8..3, 0..3), 0u8..4).prop_map(|(dirs, f)| {
        let mut p: Vec<String> = dirs.iter().map(|d| format!("d{d}")).collect();
        p.push(format!("f{f}"));
        p.join("/").into_bytes()
    })
}

fn arb_files() -> impl Strategy<Value = Files> {
    prop::collection::btree_map(arb_path(), prop::collection::vec(any::<u8>(), 0..20), 0..10)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_directory_round_trips_through_write_and_read(f in arb_files()) {
        let d = tempfile::tempdir().unwrap();
        apply(d.path(), &Files::new(), &f).unwrap();
        prop_assert_eq!(read_files(d.path()).unwrap(), f);
    }

    #[test]
    fn apply_lands_exactly_on_the_target_and_back(a in arb_files(), b in arb_files()) {
        let d = tempfile::tempdir().unwrap();
        apply(d.path(), &Files::new(), &a).unwrap();
        apply(d.path(), &a, &b).unwrap();
        prop_assert_eq!(read_files(d.path()).unwrap(), b.clone());
        apply(d.path(), &b, &a).unwrap();
        prop_assert_eq!(read_files(d.path()).unwrap(), a);
    }

    #[test]
    fn untracked_files_survive_any_transition(a in arb_files(), b in arb_files()) {
        let d = tempfile::tempdir().unwrap();
        apply(d.path(), &Files::new(), &a).unwrap();
        // A file no snapshot mentions (its name cannot collide: snapshots use f0-f3/d0-d2).
        std::fs::write(d.path().join("UNTRACKED"), b"mine").unwrap();
        apply(d.path(), &a, &b).unwrap();
        prop_assert_eq!(std::fs::read(d.path().join("UNTRACKED")).unwrap(), b"mine".to_vec());
    }
}
