//! Property tests (ticket 004): tree diffs match a brute-force comparison of
//! flattened snapshots, and unchanged subtrees are never read.

use std::collections::BTreeSet;

use object::{Object, ObjectId};
use proptest::prelude::*;
use repo::{Change, Files, Repo};
use storage::Store;

fn arb_path() -> impl Strategy<Value = Vec<u8>> {
    (prop::collection::vec(0u8..3, 0..4), 0u8..4).prop_map(|(dirs, f)| {
        let mut p: Vec<String> = dirs.iter().map(|d| format!("d{d}")).collect();
        p.push(format!("f{f}"));
        p.join("/").into_bytes()
    })
}

fn arb_files() -> impl Strategy<Value = Files> {
    prop::collection::btree_map(arb_path(), prop::collection::vec(0u8..3, 0..3), 0..12)
}

fn key(c: &Change) -> (char, Vec<u8>) {
    match c {
        Change::Added { path, .. } => ('A', path.clone()),
        Change::Removed { path, .. } => ('D', path.clone()),
        Change::Modified { path, .. } => ('M', path.clone()),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn diff_trees_matches_a_brute_force_comparison(a in arb_files(), b in arb_files()) {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        let (ta, tb) = (r.write_tree(&a).unwrap(), r.write_tree(&b).unwrap());
        let got = r.diff_trees(&ta, &tb).unwrap();

        let mut want: BTreeSet<(char, Vec<u8>)> = BTreeSet::new();
        for (p, v) in &a {
            match b.get(p) {
                None => { want.insert(('D', p.clone())); }
                Some(w) if w != v => { want.insert(('M', p.clone())); }
                _ => {}
            }
        }
        for p in b.keys() {
            if !a.contains_key(p) { want.insert(('A', p.clone())); }
        }
        let got_keys: Vec<(char, Vec<u8>)> = got.iter().map(key).collect();
        prop_assert_eq!(got_keys.iter().cloned().collect::<BTreeSet<_>>(), want);
        prop_assert_eq!(got_keys.len(), got.len(), "no path reported twice");
        let paths: Vec<&[u8]> = got.iter().map(|c| c.path()).collect();
        prop_assert!(paths.windows(2).all(|w| w[0] <= w[1]), "sorted by path");

        // The reported ids are the real blob ids on each side.
        for c in &got {
            match c {
                Change::Added { path, id } => prop_assert_eq!(*id, ObjectId::of(&Object::Blob(b[path].clone()).encode().unwrap())),
                Change::Removed { path, id } => prop_assert_eq!(*id, ObjectId::of(&Object::Blob(a[path].clone()).encode().unwrap())),
                Change::Modified { path, old, new } => {
                    prop_assert_eq!(*old, ObjectId::of(&Object::Blob(a[path].clone()).encode().unwrap()));
                    prop_assert_eq!(*new, ObjectId::of(&Object::Blob(b[path].clone()).encode().unwrap()));
                }
            }
        }
    }

    #[test]
    fn diffing_is_antisymmetric(a in arb_files(), b in arb_files()) {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Repo::open(dir.path()).unwrap();
        let (ta, tb) = (r.write_tree(&a).unwrap(), r.write_tree(&b).unwrap());
        let flip = |c: &Change| match c {
            Change::Added { path, .. } => ('D', path.clone()),
            Change::Removed { path, .. } => ('A', path.clone()),
            Change::Modified { path, .. } => ('M', path.clone()),
        };
        let forward: BTreeSet<_> = r.diff_trees(&ta, &tb).unwrap().iter().map(flip).collect();
        let backward: BTreeSet<_> = r.diff_trees(&tb, &ta).unwrap().iter().map(key).collect();
        prop_assert_eq!(forward, backward);
    }
}

#[test]
fn unchanged_subtrees_are_skipped_without_being_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = Files::new();
    for i in 0..20 {
        files.insert(
            format!("big/dir{}/f{i}", i % 4).into_bytes(),
            vec![i as u8; 64],
        );
    }
    files.insert(b"small.txt".to_vec(), b"v1".to_vec());
    let (ta, tb, big_tree, big_blobs);
    {
        let mut r = Repo::open(dir.path()).unwrap();
        ta = r.write_tree(&files).unwrap();
        files.insert(b"small.txt".to_vec(), b"v2".to_vec());
        tb = r.write_tree(&files).unwrap();
        // Find the unchanged "big" subtree and every object beneath it.
        let Object::Tree(root) = r.require(&ta).unwrap() else {
            panic!()
        };
        big_tree = root.get(b"big").unwrap().id;
        let Object::Tree(big) = r.require(&big_tree).unwrap() else {
            panic!()
        };
        let mut ids = vec![big_tree];
        for d in big.entries() {
            ids.push(d.id);
            let Object::Tree(sub) = r.require(&d.id).unwrap() else {
                panic!()
            };
            ids.extend(sub.entries().iter().map(|e| e.id));
        }
        big_blobs = ids;
    }
    // Destroy the whole unchanged subtree behind the repository's back.
    {
        let mut raw = Store::open(dir.path()).unwrap();
        for id in &big_blobs {
            let mut k = vec![b'o'];
            k.extend(id.0);
            raw.delete(k).unwrap();
        }
    }
    let r = Repo::open(dir.path()).unwrap();
    assert!(
        r.read_tree(&ta).is_err(),
        "reading the full tree needs the missing objects"
    );
    let changes = r.diff_trees(&ta, &tb).unwrap();
    assert_eq!(
        changes.len(),
        1,
        "only small.txt changed, and the diff never touched big/"
    );
    assert_eq!(changes[0].path(), b"small.txt");
    let _ = big_tree;
}
