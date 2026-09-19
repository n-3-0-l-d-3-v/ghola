//! Property tests (ticket 005): snapshot merging is an identity for trivial
//! merges, symmetric, sound in what it calls a conflict, and combines edits
//! to disjoint paths exactly; commit-level merging writes the merged tree.

use proptest::prelude::*;
use repo::{merge_files, ConflictKind, Files, MergeOutcome, Repo};

const PATHS: [&str; 5] = ["a", "b", "c/d", "c/e", "f"];

fn arb_content() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop_oneof![Just("1\n"), Just("2\n"), Just("3\n")], 0..5)
        .prop_map(|ls| ls.concat().into_bytes())
}

/// One snapshot as an optional content per path in a fixed universe.
fn arb_state() -> impl Strategy<Value = Vec<Option<Vec<u8>>>> {
    prop::collection::vec(prop::option::of(arb_content()), PATHS.len())
}

fn files(state: &[Option<Vec<u8>>]) -> Files {
    PATHS
        .iter()
        .zip(state)
        .filter_map(|(p, c)| c.as_ref().map(|c| (p.as_bytes().to_vec(), c.clone())))
        .collect()
}

fn flip(k: ConflictKind) -> ConflictKind {
    match k {
        ConflictKind::ModifyDelete { ours_deleted } => ConflictKind::ModifyDelete {
            ours_deleted: !ours_deleted,
        },
        other => other,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn identical_sides_and_one_sided_changes_are_identities(b in arb_state(), x in arb_state()) {
        let (b, x) = (files(&b), files(&x));
        for m in [
            merge_files(&b, &x, &x, "o", "t"),
            merge_files(&b, &b, &x, "o", "t"),
            merge_files(&b, &x, &b, "o", "t"),
        ] {
            prop_assert!(m.conflicts.is_empty());
            prop_assert_eq!(&m.files, &x);
        }
    }

    #[test]
    fn merging_is_symmetric(b in arb_state(), o in arb_state(), t in arb_state()) {
        let (b, o, t) = (files(&b), files(&o), files(&t));
        let ab = merge_files(&b, &o, &t, "o", "t");
        let ba = merge_files(&b, &t, &o, "t", "o");
        let flipped: Vec<_> = ba.conflicts.iter().map(|c| (c.path.clone(), flip(c.kind))).collect();
        let direct: Vec<_> = ab.conflicts.iter().map(|c| (c.path.clone(), c.kind)).collect();
        prop_assert_eq!(direct, flipped);
        if ab.conflicts.is_empty() {
            prop_assert_eq!(ab.files, ba.files);
        }
    }

    #[test]
    fn a_reported_conflict_means_both_sides_really_changed_the_path_differently(
        b in arb_state(), o in arb_state(), t in arb_state(),
    ) {
        let (b, o, t) = (files(&b), files(&o), files(&t));
        for c in merge_files(&b, &o, &t, "o", "t").conflicts {
            let (bb, oo, tt) = (b.get(&c.path), o.get(&c.path), t.get(&c.path));
            prop_assert_ne!(oo, bb);
            prop_assert_ne!(tt, bb);
            prop_assert_ne!(oo, tt);
        }
    }

    #[test]
    fn edits_to_disjoint_paths_always_combine_exactly(
        b in arb_state(),
        who in prop::collection::vec(0u8..3, PATHS.len()),
        edits in prop::collection::vec(prop::option::of(arb_content()), PATHS.len()),
    ) {
        let (mut o, mut t, mut expected) = (b.clone(), b.clone(), b.clone());
        for i in 0..PATHS.len() {
            match who[i] {
                1 => { o[i] = edits[i].clone(); expected[i] = edits[i].clone(); }
                2 => { t[i] = edits[i].clone(); expected[i] = edits[i].clone(); }
                _ => {}
            }
        }
        let m = merge_files(&files(&b), &files(&o), &files(&t), "o", "t");
        prop_assert!(m.conflicts.is_empty());
        prop_assert_eq!(m.files, files(&expected));
    }

    #[test]
    fn merge_commits_writes_the_merged_snapshot_with_two_parents(
        b in arb_state(),
        who in prop::collection::vec(0u8..3, PATHS.len()),
        edits in prop::collection::vec(prop::option::of(arb_content()), PATHS.len()),
    ) {
        let (mut o, mut t, mut expected) = (b.clone(), b.clone(), b.clone());
        for i in 0..PATHS.len() {
            match who[i] {
                1 => { o[i] = edits[i].clone(); expected[i] = edits[i].clone(); }
                2 => { t[i] = edits[i].clone(); expected[i] = edits[i].clone(); }
                _ => {}
            }
        }
        prop_assume!(files(&o) != files(&b) && files(&t) != files(&b) && files(&o) != files(&t));
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        let mut commit = |st: &[Option<Vec<u8>>], parents: Vec<_>, ts: u64| {
            let tree = r.write_tree(&files(st)).unwrap();
            r.write_commit(tree, parents, "a", &format!("c{ts}"), ts).unwrap()
        };
        let base = commit(&b, vec![], 1);
        let ours = commit(&o, vec![base], 2);
        let theirs = commit(&t, vec![base], 3);
        match r.merge_commits(&ours, &theirs, "m", "merge", 4).unwrap() {
            MergeOutcome::Merged(m) => {
                let c = r.commit_of(&m).unwrap();
                prop_assert_eq!(c.parents, vec![ours, theirs]);
                prop_assert_eq!(r.read_tree(&c.tree).unwrap(), files(&expected));
            }
            other => prop_assert!(false, "expected a clean merge, got {:?}", other),
        }
    }
}
