//! Property tests (ticket 002): arbitrary snapshots round-trip through the
//! object store (including across a real reopen), rewriting a snapshot
//! stores nothing, and changing one file adds only the objects on its path.

use std::collections::BTreeMap;

use object::{Object, ObjectId};
use proptest::prelude::*;
use repo::{Files, Repo};

/// Paths like `d1/d0/f2`: directories are `dN`, files `fN`, so a name is
/// never both (that conflict is tested separately).
fn arb_path() -> impl Strategy<Value = Vec<u8>> {
    (prop::collection::vec(0u8..3, 0..4), 0u8..4).prop_map(|(dirs, f)| {
        let mut p: Vec<String> = dirs.iter().map(|d| format!("d{d}")).collect();
        p.push(format!("f{f}"));
        p.join("/").into_bytes()
    })
}

fn arb_files() -> impl Strategy<Value = Files> {
    prop::collection::btree_map(arb_path(), prop::collection::vec(any::<u8>(), 0..30), 0..12)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn snapshots_round_trip_including_across_a_reopen(files in arb_files()) {
        let dir = tempfile::tempdir().unwrap();
        let root = {
            let mut r = Repo::open(dir.path()).unwrap();
            let id = r.write_tree(&files).unwrap();
            prop_assert_eq!(&r.read_tree(&id).unwrap(), &files);
            id
        };
        let r = Repo::open(dir.path()).unwrap();
        prop_assert_eq!(&r.read_tree(&root).unwrap(), &files);
        // Every stored object still verifies against its own id.
        for id in r.object_ids() {
            prop_assert!(r.get(&id).unwrap().is_some());
        }
    }

    #[test]
    fn rewriting_a_snapshot_stores_nothing_new(files in arb_files()) {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        let a = r.write_tree(&files).unwrap();
        let n = r.stats();
        let b = r.write_tree(&files).unwrap();
        prop_assert_eq!(a, b);
        prop_assert_eq!(r.stats(), n);
    }

    #[test]
    fn changing_one_file_adds_only_the_objects_on_its_path(
        files in arb_files().prop_filter("non-empty", |f| !f.is_empty()),
        which in any::<prop::sample::Index>(),
        new_content in prop::collection::vec(any::<u8>(), 31..40), // never equals an old content (len < 30)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        r.write_tree(&files).unwrap();
        let before = r.stats().objects;
        let path = files.keys().nth(which.index(files.len())).unwrap().clone();
        let components = path.iter().filter(|&&b| b == b'/').count() + 1;
        let mut changed = files.clone();
        changed.insert(path, new_content);
        r.write_tree(&changed).unwrap();
        // One new blob, plus one new tree per directory level including the root.
        prop_assert!(r.stats().objects - before <= components + 1);
    }

    #[test]
    fn distinct_snapshots_get_distinct_root_ids(a in arb_files(), b in arb_files()) {
        prop_assume!(a != b);
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        prop_assert_ne!(r.write_tree(&a).unwrap(), r.write_tree(&b).unwrap());
    }
}

#[test]
fn history_deduplicates_without_storing_diffs_and_here_are_the_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Repo::open(dir.path()).unwrap();
    let mut files: BTreeMap<Vec<u8>, Vec<u8>> = (0..50)
        .map(|i| {
            (
                format!("dir{}/file{i}.txt", i % 5).into_bytes(),
                vec![i as u8; 1024],
            )
        })
        .collect();
    let commits = 100usize;
    let mut roots = Vec::new();
    for c in 0..commits {
        let key = format!("dir{}/file{}.txt", (c % 50) % 5, c % 50).into_bytes();
        files.insert(key, vec![(c % 251) as u8 ^ 0x55; 1024]);
        roots.push(r.write_tree(&files).unwrap());
    }
    let stats = r.stats();
    let naive_bytes = commits * (50 * 1024);
    eprintln!(
        "{commits} commits of a 50-file (50 KiB) tree, one file edited per commit: \
         {} objects, {} bytes stored vs {} bytes if every commit stored a full copy ({:.1}%)",
        stats.objects,
        stats.bytes,
        naive_bytes,
        100.0 * stats.bytes as f64 / naive_bytes as f64
    );
    assert!(
        stats.bytes * 10 < naive_bytes,
        "deduplication should keep this under 10% of naive"
    );
    // Every historical snapshot is still fully readable: no diffs to replay.
    for (c, root) in roots.iter().enumerate() {
        assert_eq!(r.read_tree(root).unwrap().len(), 50, "commit {c}");
    }
    let _ = Object::Blob(vec![]).id().map(|_: ObjectId| ());
}
