//! Property tests (ticket 005) for three-way merge: the alignment is
//! lossless, trivial merges are identities, merging is symmetric, and edits
//! built to be disjoint (or built to collide) merge (or conflict) exactly as
//! constructed.

use diff::*;
use proptest::prelude::*;

fn arb_seq() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0u8..4, 0..24)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_alignment_is_lossless(base in arb_seq(), ours in arb_seq(), theirs in arb_seq()) {
        let m = merge3(&base, &ours, &theirs);
        let (mut b, mut o, mut t): (Vec<u8>, Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new(), Vec::new());
        for c in &m.chunks {
            match c {
                Chunk::Same(v) => { b.extend(v); o.extend(v); t.extend(v); }
                Chunk::Changed { base, ours, theirs } => {
                    prop_assert_ne!(c.kind(), Kind::Same);
                    b.extend(base); o.extend(ours); t.extend(theirs);
                }
            }
        }
        prop_assert_eq!(b, base);
        prop_assert_eq!(o, ours);
        prop_assert_eq!(t, theirs);
    }

    #[test]
    fn merging_identical_sides_returns_that_side(base in arb_seq(), x in arb_seq()) {
        prop_assert_eq!(merge3(&base, &x, &x).clean(), Some(x));
    }

    #[test]
    fn a_one_sided_change_applies_cleanly(base in arb_seq(), x in arb_seq()) {
        prop_assert_eq!(merge3(&base, &base, &x).clean(), Some(x.clone()));
        prop_assert_eq!(merge3(&base, &x, &base).clean(), Some(x));
    }

    #[test]
    fn merging_is_symmetric(base in arb_seq(), ours in arb_seq(), theirs in arb_seq()) {
        let a = merge3(&base, &ours, &theirs);
        let b = merge3(&base, &theirs, &ours);
        prop_assert_eq!(a.has_conflicts(), b.has_conflicts());
        prop_assert_eq!(a.clean(), b.clean());
        // Swapping sides swaps every changed region's sides.
        prop_assert_eq!(a.chunks.len(), b.chunks.len());
        for (x, y) in a.chunks.iter().zip(&b.chunks) {
            match (x, y) {
                (Chunk::Same(p), Chunk::Same(q)) => prop_assert_eq!(p, q),
                (Chunk::Changed { base: b1, ours: o1, theirs: t1 },
                 Chunk::Changed { base: b2, ours: o2, theirs: t2 }) => {
                    prop_assert_eq!(b1, b2);
                    prop_assert_eq!(o1, t2);
                    prop_assert_eq!(t1, o2);
                }
                _ => prop_assert!(false, "chunk shapes differ"),
            }
        }
    }
}

/// A base made of segments (elements 0..4) joined by a unique, never-edited
/// separator (200); edits replace a whole segment by fresh elements (>= 10),
/// possibly none. With fresh replacement values the minimal alignment is
/// unique, so the edit locations are exactly the segments chosen.
#[derive(Debug, Clone)]
struct Scenario {
    segments: Vec<Vec<u8>>,
    /// Per segment: 0 untouched, 1 ours only, 2 theirs only, 3 both (colliding).
    who: Vec<u8>,
    ours_new: Vec<Vec<u8>>,
    theirs_new: Vec<Vec<u8>>,
}

fn arb_scenario() -> impl Strategy<Value = Scenario> {
    (1usize..7).prop_flat_map(|n| {
        (
            prop::collection::vec(prop::collection::vec(0u8..4, 1..4), n),
            prop::collection::vec(0u8..4, n),
            prop::collection::vec(prop::collection::vec(10u8..14, 0..3), n),
            prop::collection::vec(prop::collection::vec(20u8..24, 1..3), n),
        )
            .prop_map(|(segments, who, ours_new, theirs_new)| Scenario {
                segments,
                who,
                ours_new,
                theirs_new,
            })
    })
}

fn join(segs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        if i > 0 {
            out.push(200);
        }
        out.extend(s);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn disjoint_edits_merge_cleanly_and_collisions_conflict_exactly_where_built(sc in arb_scenario()) {
        let base = join(&sc.segments);
        let (mut ours, mut theirs, mut expected) = (Vec::new(), Vec::new(), Vec::new());
        let mut collisions = Vec::new();
        for (i, seg) in sc.segments.iter().enumerate() {
            let o = if matches!(sc.who[i], 1 | 3) { sc.ours_new[i].clone() } else { seg.clone() };
            let t = if matches!(sc.who[i], 2 | 3) { sc.theirs_new[i].clone() } else { seg.clone() };
            let e = match sc.who[i] {
                1 => o.clone(),
                2 => t.clone(),
                3 => { collisions.push((o.clone(), t.clone())); seg.clone() }
                _ => seg.clone(),
            };
            ours.push(o);
            theirs.push(t);
            expected.push(e);
        }
        let (ours, theirs, expected) = (join(&ours), join(&theirs), join(&expected));

        let m = merge3(&base, &ours, &theirs);
        let conflicts: Vec<(Vec<u8>, Vec<u8>)> = m
            .chunks
            .iter()
            .filter_map(|c| match (c, c.kind()) {
                (Chunk::Changed { ours, theirs, .. }, Kind::Conflict) => Some((ours.clone(), theirs.clone())),
                _ => None,
            })
            .collect();
        prop_assert_eq!(&conflicts, &collisions, "conflicts appear exactly where both sides edited the same segment");
        if collisions.is_empty() {
            prop_assert_eq!(m.clean(), Some(expected));
        } else {
            prop_assert_eq!(m.clean(), None);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn marker_rendering_matches_the_conflict_count(
        base in prop::collection::vec(prop_oneof![Just("a\n"), Just("b\n"), Just("c\n")], 0..8),
        ours in prop::collection::vec(prop_oneof![Just("a\n"), Just("b\n"), Just("X\n")], 0..8),
        theirs in prop::collection::vec(prop_oneof![Just("a\n"), Just("c\n"), Just("Y\n")], 0..8),
    ) {
        let (b, o, t) = (base.concat(), ours.concat(), theirs.concat());
        let m = merge_text(b.as_bytes(), o.as_bytes(), t.as_bytes());
        let out = String::from_utf8(render_with_markers(&m, "ours", "theirs")).unwrap();
        let n = m.chunks.iter().filter(|c| c.kind() == Kind::Conflict).count();
        prop_assert_eq!(out.matches("<<<<<<< ours\n").count(), n);
        prop_assert_eq!(out.matches("=======\n").count(), n);
        prop_assert_eq!(out.matches(">>>>>>> theirs\n").count(), n);
        if n == 0 {
            let clean = m.clean().unwrap().concat();
            prop_assert_eq!(out.as_bytes(), clean.as_slice());
        }
    }
}
