//! Property tests (ticket 004): applying a diff reconstructs the target, the
//! script is minimal (checked against an O(nm) LCS dynamic-programming
//! reference), patches invert, and unified output is self-consistent.

use diff::*;
use proptest::prelude::*;

/// Small alphabet so long common subsequences and repeats are common, the
/// hard case for diff algorithms.
fn arb_seq() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0u8..4, 0..40)
}

fn lcs_len(a: &[u8], b: &[u8]) -> usize {
    let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            t[i][j] = if a[i - 1] == b[j - 1] {
                t[i - 1][j - 1] + 1
            } else {
                t[i - 1][j].max(t[i][j - 1])
            };
        }
    }
    t[a.len()][b.len()]
}

/// Files made only of complete lines, so the unified body has no
/// "no newline" marker lines.
fn arb_lines() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop_oneof![Just(""), Just("a"), Just("b")], 0..12).prop_map(|ls| {
        ls.iter()
            .flat_map(|l| {
                format!(
                    "{l}
"
                )
                .into_bytes()
            })
            .collect()
    })
}

fn arb_text() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop_oneof![Just(b'\n'), Just(b'a'), Just(b'b')], 0..40)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn applying_the_diff_reconstructs_the_target(a in arb_seq(), b in arb_seq()) {
        let p = diff(&a, &b);
        prop_assert_eq!(p.apply(&a).unwrap(), b);
    }

    #[test]
    fn the_edit_script_is_minimal(a in arb_seq(), b in arb_seq()) {
        let p = diff(&a, &b);
        prop_assert_eq!(p.edit_distance(), a.len() + b.len() - 2 * lcs_len(&a, &b));
    }

    #[test]
    fn a_patch_inverts(a in arb_seq(), b in arb_seq()) {
        let p = diff(&a, &b);
        let back = p.invert(&a).unwrap();
        prop_assert_eq!(back.apply(&b).unwrap(), a);
    }

    #[test]
    fn diffing_a_sequence_with_itself_is_a_single_keep(a in arb_seq()) {
        let p = diff(&a, &a);
        if a.is_empty() {
            prop_assert!(p.ops.is_empty());
        } else {
            prop_assert_eq!(p.ops, vec![PatchOp::Equal(a.len())]);
        }
    }

    #[test]
    fn scripts_are_coalesced_with_no_empty_or_repeated_runs(a in arb_seq(), b in arb_seq()) {
        let p = diff(&a, &b);
        for w in p.ops.windows(2) {
            let same = matches!(
                (&w[0], &w[1]),
                (PatchOp::Equal(_), PatchOp::Equal(_))
                    | (PatchOp::Delete(_), PatchOp::Delete(_))
                    | (PatchOp::Insert(_), PatchOp::Insert(_))
            );
            prop_assert!(!same, "adjacent runs of the same kind: {:?}", p.ops);
        }
        for op in &p.ops {
            match op {
                PatchOp::Equal(n) | PatchOp::Delete(n) => prop_assert!(*n > 0),
                PatchOp::Insert(v) => prop_assert!(!v.is_empty()),
            }
        }
    }

    #[test]
    fn text_patches_reconstruct_exactly(a in arb_text(), b in arb_text()) {
        let p = diff_text(&a, &b);
        prop_assert_eq!(apply_text(&a, &p).unwrap(), b);
    }

    #[test]
    fn lines_always_concatenate_back(a in arb_text()) {
        prop_assert_eq!(split_lines(&a).concat(), a);
    }

    #[test]
    fn unified_output_is_self_consistent(a in arb_text(), b in arb_text(), ctx in 0usize..4) {
        let out = unified("a", "b", &a, &b, ctx);
        if a == b {
            prop_assert_eq!(out, "");
            return Ok(());
        }
        // Every hunk header's counts match the lines that follow it.
        let mut lines = out.lines().skip(2).peekable();
        while let Some(h) = lines.next() {
            prop_assert!(h.starts_with("@@ -"), "unexpected line {:?}", h);
            let nums: Vec<usize> = h
                .trim_start_matches("@@ ")
                .trim_end_matches(" @@")
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect();
            let (a_count, b_count) = (nums[1], nums[3]);
            let (mut sa, mut sb) = (0, 0);
            while let Some(l) = lines.peek() {
                if l.starts_with("@@ -") { break; }
                match l.as_bytes().first() {
                    Some(b' ') => { sa += 1; sb += 1; }
                    Some(b'-') => sa += 1,
                    Some(b'+') => sb += 1,
                    _ => {}
                }
                lines.next();
            }
            prop_assert_eq!((sa, sb), (a_count, b_count));
        }
    }

    #[test]
    fn with_enough_context_the_unified_body_reproduces_both_files(a in arb_lines(), b in arb_lines()) {
        prop_assume!(a != b);
        let out = unified("a", "b", &a, &b, 1000);
        let (mut from_a, mut from_b): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        for l in out.lines().skip(3) {
            let (tag, rest) = l.split_at(1);
            let mut line = rest.as_bytes().to_vec();
            line.push(10);
            match tag {
                " " => { from_a.extend(&line); from_b.extend(&line); }
                "-" => from_a.extend(&line),
                "+" => from_b.extend(&line),
                _ => prop_assert!(false, "bad line {:?}", l),
            }
        }
        prop_assert_eq!(from_a, a);
        prop_assert_eq!(from_b, b);
    }
}
