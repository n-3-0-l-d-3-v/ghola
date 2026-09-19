//! Line-oriented text diffs and the unified diff format. Lines keep their
//! trailing `\n`, so a file that does and one that does not end in a newline
//! are genuinely different, and concatenating the lines gives the original
//! bytes back exactly.

use crate::myers::{diff, Patch, PatchOp};

/// Splits into lines, each including its terminating `\n` (the last line
/// may have none).
pub fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&b| b == b'\n').collect()
}

/// A minimal line-level patch from `a` to `b`.
pub fn diff_text(a: &[u8], b: &[u8]) -> Patch<Vec<u8>> {
    let (la, lb): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (
        split_lines(a).into_iter().map(<[u8]>::to_vec).collect(),
        split_lines(b).into_iter().map(<[u8]>::to_vec).collect(),
    );
    diff(&la, &lb)
}

/// Applies a line patch to text.
pub fn apply_text(a: &[u8], patch: &Patch<Vec<u8>>) -> Result<Vec<u8>, crate::myers::PatchError> {
    let lines: Vec<Vec<u8>> = split_lines(a).into_iter().map(<[u8]>::to_vec).collect();
    Ok(patch.apply(&lines)?.concat())
}

struct Line<'a> {
    tag: u8,
    text: &'a [u8],
    a_before: usize,
    b_before: usize,
}

/// Renders a unified diff (`--- / +++ / @@ -a,b +c,d @@`) with `context`
/// unchanged lines around each change. Empty if the inputs are identical.
pub fn unified(a_label: &str, b_label: &str, a: &[u8], b: &[u8], context: usize) -> String {
    let (la, lb) = (split_lines(a), split_lines(b));
    let patch = diff(&la, &lb);

    let mut lines: Vec<Line<'_>> = Vec::new();
    let (mut ai, mut bi) = (0, 0);
    for op in &patch.ops {
        match op {
            PatchOp::Equal(n) => {
                for _ in 0..*n {
                    lines.push(Line {
                        tag: b' ',
                        text: la[ai],
                        a_before: ai,
                        b_before: bi,
                    });
                    ai += 1;
                    bi += 1;
                }
            }
            PatchOp::Delete(n) => {
                for _ in 0..*n {
                    lines.push(Line {
                        tag: b'-',
                        text: la[ai],
                        a_before: ai,
                        b_before: bi,
                    });
                    ai += 1;
                }
            }
            PatchOp::Insert(items) => {
                for _ in items {
                    lines.push(Line {
                        tag: b'+',
                        text: lb[bi],
                        a_before: ai,
                        b_before: bi,
                    });
                    bi += 1;
                }
            }
        }
    }

    // Windows of context around each change, merged when they touch.
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.tag == b' ' {
            continue;
        }
        let (start, end) = (
            i.saturating_sub(context),
            (i + context + 1).min(lines.len()),
        );
        match windows.last_mut() {
            Some(w) if start <= w.1 => w.1 = w.1.max(end),
            _ => windows.push((start, end)),
        }
    }
    if windows.is_empty() {
        return String::new();
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend(format!("--- {a_label}\n+++ {b_label}\n").bytes());
    for (s, e) in windows {
        let hunk = &lines[s..e];
        let a_count = hunk.iter().filter(|l| l.tag != b'+').count();
        let b_count = hunk.iter().filter(|l| l.tag != b'-').count();
        let a_start = if a_count == 0 {
            hunk[0].a_before
        } else {
            hunk[0].a_before + 1
        };
        let b_start = if b_count == 0 {
            hunk[0].b_before
        } else {
            hunk[0].b_before + 1
        };
        out.extend(format!("@@ -{a_start},{a_count} +{b_start},{b_count} @@\n").bytes());
        for l in hunk {
            out.push(l.tag);
            out.extend(l.text);
            if !l.text.ends_with(b"\n") {
                out.extend(b"\n\\ No newline at end of file\n");
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_terminators_and_concatenate_back() {
        for s in ["", "a", "a\n", "a\nb", "a\n\nb\n", "\n\n"] {
            let joined: Vec<u8> = split_lines(s.as_bytes()).concat();
            assert_eq!(joined, s.as_bytes());
        }
        assert_eq!(split_lines(b"a\nb").len(), 2);
    }

    #[test]
    fn a_missing_final_newline_is_a_real_difference() {
        let p = diff_text(b"x\ny", b"x\ny\n");
        assert_ne!(p.edit_distance(), 0);
        assert_eq!(apply_text(b"x\ny", &p).unwrap(), b"x\ny\n");
    }

    #[test]
    fn unified_matches_a_hand_checked_example() {
        let out = unified("a", "b", b"one\ntwo\nthree\n", b"one\n2\nthree\n", 3);
        assert_eq!(
            out,
            "--- a\n+++ b\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"
        );
    }

    #[test]
    fn identical_files_produce_no_diff() {
        assert_eq!(unified("a", "b", b"same\n", b"same\n", 3), "");
    }

    #[test]
    fn distant_changes_make_separate_hunks_and_near_ones_merge() {
        let a: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let mut b = a.clone();
        b = b.replace("l2\n", "L2\n").replace("l19\n", "L19\n");
        let out = unified("a", "b", a.as_bytes(), b.as_bytes(), 1);
        assert_eq!(out.matches("@@ -").count(), 2, "{out}");
        let merged = unified("a", "b", a.as_bytes(), b.as_bytes(), 20);
        assert_eq!(merged.matches("@@ -").count(), 1);
    }

    #[test]
    fn a_pure_insertion_at_the_top_uses_zero_start() {
        let out = unified("a", "b", b"", b"new\n", 3);
        assert_eq!(out, "--- a\n+++ b\n@@ -0,0 +1,1 @@\n+new\n");
    }

    #[test]
    fn the_no_newline_marker_is_emitted() {
        let out = unified("a", "b", b"x", b"y", 3);
        assert!(out.contains("-x\n\\ No newline at end of file\n"), "{out}");
        assert!(out.contains("+y\n\\ No newline at end of file\n"), "{out}");
    }

    #[test]
    fn hunks_whose_context_windows_merely_touch_are_merged() {
        // Changes at lines 2 and 5 with one line of context: the windows
        // (lines 1-3 and 4-6) touch with no gap, so this is one hunk.
        let a = b"1
2
3
4
5
6
";
        let b = b"1
X
3
4
Y
6
";
        assert_eq!(unified("a", "b", a, b, 1).matches("@@ -").count(), 1);
    }
}
