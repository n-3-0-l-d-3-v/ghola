//! Property tests (ticket 001): SHA-256 agrees with the `sha2` reference for
//! arbitrary inputs and arbitrary chunkings; objects round-trip; decoding
//! never panics; and one logical object has exactly one encoding.

use object::*;
use proptest::prelude::*;
use sha2::Digest;

fn arb_id() -> impl Strategy<Value = ObjectId> {
    any::<[u8; 32]>().prop_map(ObjectId)
}

fn arb_name() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(1u8..=255, 1..8).prop_filter("valid name", |n| {
        n != b"." && n != b".." && !n.contains(&b'/')
    })
}

fn arb_tree() -> impl Strategy<Value = Tree> {
    prop::collection::btree_map(arb_name(), (any::<bool>(), arb_id()), 0..8).prop_map(|m| {
        Tree::new(
            m.into_iter()
                .map(|(name, (dir, id))| TreeEntry {
                    name,
                    kind: if dir { EntryKind::Dir } else { EntryKind::File },
                    id,
                })
                .collect(),
        )
        .unwrap()
    })
}

fn arb_commit() -> impl Strategy<Value = Commit> {
    (
        arb_id(),
        prop::collection::vec(arb_id(), 0..4),
        any::<u64>(),
        ".{0,20}",
        ".{0,60}",
    )
        .prop_map(|(tree, parents, timestamp, author, message)| Commit {
            tree,
            parents,
            timestamp,
            author,
            message,
        })
}

fn arb_object() -> impl Strategy<Value = Object> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..100).prop_map(Object::Blob),
        arb_tree().prop_map(Object::Tree),
        arb_commit().prop_map(Object::Commit),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn sha256_matches_the_reference_for_any_input(data in prop::collection::vec(any::<u8>(), 0..600)) {
        let want: [u8; 32] = sha2::Sha256::digest(&data).into();
        prop_assert_eq!(sha256(&data), want);
    }

    #[test]
    fn sha256_is_independent_of_how_the_input_is_chunked(
        data in prop::collection::vec(any::<u8>(), 0..400),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
    ) {
        let mut points: Vec<usize> = cuts.iter().map(|c| c.index(data.len() + 1)).collect();
        points.sort_unstable();
        let mut h = Sha256::new();
        let mut last = 0;
        for p in points {
            h.update(&data[last..p]);
            last = p;
        }
        h.update(&data[last..]);
        prop_assert_eq!(h.finalize(), sha256(&data));
    }

    #[test]
    fn objects_round_trip_and_ids_are_stable(o in arb_object()) {
        let bytes = o.encode().unwrap();
        prop_assert_eq!(Object::decode(&bytes), Ok(o.clone()));
        prop_assert_eq!(o.id().unwrap(), ObjectId::of(&bytes));
    }

    #[test]
    fn one_logical_object_has_one_encoding(o in arb_object()) {
        // decode(encode(o)) re-encodes to identical bytes: there is no second
        // spelling of the same object for the decoder to accept.
        let bytes = o.encode().unwrap();
        let again = Object::decode(&bytes).unwrap().encode().unwrap();
        prop_assert_eq!(bytes, again);
    }

    #[test]
    fn decoding_arbitrary_bytes_never_panics_and_accepted_input_is_canonical(
        bytes in prop::collection::vec(any::<u8>(), 0..120),
    ) {
        if let Ok(o) = Object::decode(&bytes) {
            prop_assert_eq!(o.encode().unwrap(), bytes);
        }
    }

    #[test]
    fn tree_ids_do_not_depend_on_entry_order(t in arb_tree(), seed in any::<u64>()) {
        let mut entries: Vec<TreeEntry> = t.entries().to_vec();
        // Deterministic shuffle from the seed.
        let mut s = seed;
        for i in (1..entries.len()).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            entries.swap(i, (s >> 33) as usize % (i + 1));
        }
        let shuffled = Tree::new(entries).unwrap();
        prop_assert_eq!(Object::Tree(shuffled).id().unwrap(), Object::Tree(t).id().unwrap());
    }

    #[test]
    fn different_blobs_get_different_ids(a in prop::collection::vec(any::<u8>(), 0..50), b in prop::collection::vec(any::<u8>(), 0..50)) {
        prop_assume!(a != b);
        prop_assert_ne!(Object::Blob(a).id().unwrap(), Object::Blob(b).id().unwrap());
    }
}
