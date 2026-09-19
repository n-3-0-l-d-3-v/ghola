//! Three-way merge of sequences (ticket 005).
//!
//! Given a common ancestor `base` and two descendants `ours` and `theirs`,
//! the merge aligns all three into regions: `Same` (identical in all three)
//! and `Changed { base, ours, theirs }` (some side differs). Concatenating
//! each column of the regions gives back exactly `base`, `ours` and `theirs`,
//! so the alignment is lossless by construction and that is property-tested.
//!
//! A region is then classified:
//! - only ours changed it, or only theirs did: take the changed side;
//! - both made the identical change: take it once;
//! - both changed it differently: a conflict.
//!
//! Two changes conflict when their base ranges intersect *including
//! touching* (closed intervals), so two edits to adjacent lines, or two
//! insertions at the same point, are reported rather than silently
//! combined in an arbitrary order. That is deliberately conservative: a
//! false conflict costs a human a glance, a false clean merge can silently
//! ship the wrong code.

use crate::myers::{diff, PatchOp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk<T> {
    Same(Vec<T>),
    Changed {
        base: Vec<T>,
        ours: Vec<T>,
        theirs: Vec<T>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Same,
    OnlyOurs,
    OnlyTheirs,
    /// Both sides made the identical change.
    Both,
    Conflict,
}

impl<T: PartialEq> Chunk<T> {
    pub fn kind(&self) -> Kind {
        match self {
            Chunk::Same(_) => Kind::Same,
            Chunk::Changed { base, ours, theirs } => {
                if ours == theirs {
                    Kind::Both
                } else if theirs == base {
                    Kind::OnlyOurs
                } else if ours == base {
                    Kind::OnlyTheirs
                } else {
                    Kind::Conflict
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge3<T> {
    pub chunks: Vec<Chunk<T>>,
}

/// A replacement of base range `start..end` by `new` (a pure insertion has
/// `start == end`, a pure deletion an empty `new`).
struct Hunk<T> {
    start: usize,
    end: usize,
    new: Vec<T>,
}

fn hunks<T: Eq + Clone>(base: &[T], other: &[T]) -> Vec<Hunk<T>> {
    let mut out: Vec<Hunk<T>> = Vec::new();
    let mut pos = 0;
    let mut open: Option<Hunk<T>> = None;
    for op in diff(base, other).ops {
        match op {
            PatchOp::Equal(n) => {
                out.extend(open.take());
                pos += n;
            }
            PatchOp::Delete(n) => {
                let h = open.get_or_insert(Hunk {
                    start: pos,
                    end: pos,
                    new: Vec::new(),
                });
                h.end += n;
                pos += n;
            }
            PatchOp::Insert(items) => {
                let h = open.get_or_insert(Hunk {
                    start: pos,
                    end: pos,
                    new: Vec::new(),
                });
                h.new.extend(items);
            }
        }
    }
    out.extend(open);
    out
}

/// One side's version of base range `rs..re`, applying that side's hunks
/// which fall inside it.
fn side_view<T: Clone>(base: &[T], hs: &[Hunk<T>], rs: usize, re: usize) -> Vec<T> {
    let mut out = Vec::new();
    let mut cur = rs;
    for h in hs.iter().filter(|h| h.start >= rs && h.end <= re) {
        out.extend_from_slice(&base[cur..h.start]);
        out.extend(h.new.iter().cloned());
        cur = h.end;
    }
    out.extend_from_slice(&base[cur..re]);
    out
}

pub fn merge3<T: Eq + Clone>(base: &[T], ours: &[T], theirs: &[T]) -> Merge3<T> {
    let (ho, ht) = (hunks(base, ours), hunks(base, theirs));
    let (mut i, mut j) = (0, 0);
    let mut pos = 0;
    let mut chunks = Vec::new();
    while i < ho.len() || j < ht.len() {
        let start = match (ho.get(i), ht.get(j)) {
            (Some(a), Some(b)) => a.start.min(b.start),
            (Some(a), None) => a.start,
            (None, Some(b)) => b.start,
            (None, None) => unreachable!(),
        };
        if start > pos {
            chunks.push(Chunk::Same(base[pos..start].to_vec()));
        }
        // Grow a cluster of every hunk (from either side) whose closed
        // range touches the region so far.
        let (mut end, i0, j0) = (start, i, j);
        loop {
            let mut grew = false;
            if let Some(h) = ho.get(i).filter(|h| h.start <= end) {
                end = end.max(h.end);
                i += 1;
                grew = true;
            }
            if let Some(h) = ht.get(j).filter(|h| h.start <= end) {
                end = end.max(h.end);
                j += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        chunks.push(Chunk::Changed {
            base: base[start..end].to_vec(),
            ours: side_view(base, &ho[i0..i], start, end),
            theirs: side_view(base, &ht[j0..j], start, end),
        });
        pos = end;
    }
    if pos < base.len() {
        chunks.push(Chunk::Same(base[pos..].to_vec()));
    }
    Merge3 { chunks }
}

impl<T: Eq + Clone> Merge3<T> {
    pub fn has_conflicts(&self) -> bool {
        self.chunks.iter().any(|c| c.kind() == Kind::Conflict)
    }

    /// The merged sequence if there is no conflict.
    pub fn clean(&self) -> Option<Vec<T>> {
        let mut out = Vec::new();
        for c in &self.chunks {
            match (c, c.kind()) {
                (Chunk::Same(v), _) => out.extend(v.iter().cloned()),
                (_, Kind::Conflict) => return None,
                (Chunk::Changed { ours, .. }, Kind::OnlyOurs | Kind::Both) => {
                    out.extend(ours.iter().cloned())
                }
                (Chunk::Changed { theirs, .. }, Kind::OnlyTheirs) => {
                    out.extend(theirs.iter().cloned())
                }
                (Chunk::Changed { .. }, Kind::Same) => unreachable!(),
            }
        }
        Some(out)
    }
}

/// Merges text line by line. Conflicts can be rendered with markers by
/// `render_with_markers`.
pub fn merge_text(base: &[u8], ours: &[u8], theirs: &[u8]) -> Merge3<Vec<u8>> {
    let lines = |b: &[u8]| -> Vec<Vec<u8>> {
        b.split_inclusive(|&c| c == b'\n')
            .map(<[u8]>::to_vec)
            .collect()
    };
    merge3(&lines(base), &lines(ours), &lines(theirs))
}

/// The merged text with `<<<<<<<` / `=======` / `>>>>>>>` markers around
/// each conflict. A conflicting side whose last line lacks a newline gets
/// one so the markers stay on their own lines.
pub fn render_with_markers(m: &Merge3<Vec<u8>>, ours_label: &str, theirs_label: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let push_side = |out: &mut Vec<u8>, side: &[Vec<u8>]| {
        for l in side {
            out.extend(l);
        }
        if side.last().is_some_and(|l| !l.ends_with(b"\n")) {
            out.push(b'\n');
        }
    };
    for c in &m.chunks {
        match (c, c.kind()) {
            (Chunk::Same(v), _) => out.extend(v.concat()),
            (Chunk::Changed { ours, .. }, Kind::OnlyOurs | Kind::Both) => out.extend(ours.concat()),
            (Chunk::Changed { theirs, .. }, Kind::OnlyTheirs) => out.extend(theirs.concat()),
            (Chunk::Changed { ours, theirs, .. }, Kind::Conflict) => {
                out.extend(format!("<<<<<<< {ours_label}\n").bytes());
                push_side(&mut out, ours);
                out.extend(b"=======\n");
                push_side(&mut out, theirs);
                out.extend(format!(">>>>>>> {theirs_label}\n").bytes());
            }
            (Chunk::Changed { .. }, Kind::Same) => unreachable!(),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(base: &str, ours: &str, theirs: &str) -> Merge3<Vec<u8>> {
        merge_text(base.as_bytes(), ours.as_bytes(), theirs.as_bytes())
    }

    fn clean(base: &str, ours: &str, theirs: &str) -> Option<String> {
        m(base, ours, theirs)
            .clean()
            .map(|v| String::from_utf8(v.concat()).unwrap())
    }

    #[test]
    fn changes_in_different_places_merge_cleanly() {
        assert_eq!(
            clean("a\nb\nc\nd\ne\n", "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n").as_deref(),
            Some("A\nb\nc\nd\nE\n")
        );
    }

    #[test]
    fn an_identical_change_on_both_sides_is_taken_once() {
        assert_eq!(
            clean("a\nb\n", "a\nX\n", "a\nX\n").as_deref(),
            Some("a\nX\n")
        );
    }

    #[test]
    fn different_changes_to_the_same_line_conflict() {
        let merged = m("a\nb\nc\n", "a\nOURS\nc\n", "a\nTHEIRS\nc\n");
        assert!(merged.has_conflicts());
        assert_eq!(
            String::from_utf8(render_with_markers(&merged, "ours", "theirs")).unwrap(),
            "a\n<<<<<<< ours\nOURS\n=======\nTHEIRS\n>>>>>>> theirs\nc\n"
        );
        assert_eq!(merged.clean(), None);
    }

    #[test]
    fn adjacent_edits_are_reported_not_silently_combined() {
        assert!(m("a\nb\nc\n", "A\nb\nc\n", "a\nB\nc\n").has_conflicts());
    }

    #[test]
    fn one_side_deleting_and_the_other_editing_the_same_line_conflicts() {
        assert!(m("a\nb\nc\n", "a\nc\n", "a\nB\nc\n").has_conflicts());
    }

    #[test]
    fn a_deletion_far_from_an_edit_merges_cleanly() {
        assert_eq!(
            clean("1\n2\n3\n4\n5\n", "1\n3\n4\n5\n", "1\n2\n3\n4\nFIVE\n").as_deref(),
            Some("1\n3\n4\nFIVE\n")
        );
    }

    #[test]
    fn two_insertions_at_the_same_point_conflict_unless_identical() {
        assert!(m("a\nc\n", "a\nX\nc\n", "a\nY\nc\n").has_conflicts());
        assert_eq!(
            clean("a\nc\n", "a\nX\nc\n", "a\nX\nc\n").as_deref(),
            Some("a\nX\nc\n")
        );
    }

    #[test]
    fn an_empty_base_with_two_different_files_conflicts() {
        assert!(m("", "x\n", "y\n").has_conflicts());
        assert_eq!(clean("", "x\n", "x\n").as_deref(), Some("x\n"));
        assert_eq!(clean("", "", "y\n").as_deref(), Some("y\n"));
    }

    #[test]
    fn markers_stay_on_their_own_lines_when_a_side_lacks_a_final_newline() {
        let merged = m("a\n", "ours", "theirs");
        let out = String::from_utf8(render_with_markers(&merged, "o", "t")).unwrap();
        assert_eq!(out, "<<<<<<< o\nours\n=======\ntheirs\n>>>>>>> t\n");
    }
}
