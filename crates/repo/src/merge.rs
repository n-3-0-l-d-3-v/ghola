//! Merging snapshots and commits (ticket 005), built on the line-level
//! three-way merge in the `diff` crate.
//!
//! Per path, with `base`, `ours`, `theirs` each present or absent:
//! - identical on both sides: take it (including both deleting it);
//! - only one side changed it: take that side (including a deletion);
//! - both changed it differently: text is merged line by line (a clean
//!   merge is used, a conflicting one is written out with conflict markers
//!   and reported); binary files, and one side deleting what the other
//!   modified, are reported as conflicts and keep the surviving version.
//!
//! `merge_commits` decides fast-forward / already-up-to-date / real merge
//! and, for a real clean merge, writes the two-parent merge commit. It never
//! moves a ref; the caller does.
//!
//! Limitation: when histories have several best common ancestors (a
//! criss-cross merge), one merge base is chosen deterministically instead of
//! recursively merging the bases as git's `recursive`/`ort` strategies do.

use std::collections::BTreeSet;

use diff::{merge_text, render_with_markers};
use object::ObjectId;

use crate::{Files, Repo, RepoError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides edited the same lines differently.
    Content,
    /// Both sides added the file with different contents.
    BothAdded,
    /// One side deleted the file, the other modified it.
    ModifyDelete { ours_deleted: bool },
    /// A binary file changed differently on both sides.
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathConflict {
    pub path: Vec<u8>,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMerge {
    /// The merged snapshot. Conflicting text files contain markers.
    pub files: Files,
    /// Sorted by path; empty means the merge was clean.
    pub conflicts: Vec<PathConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// `theirs` is already contained in `ours`.
    AlreadyUpToDate,
    /// `ours` is an ancestor of `theirs`: just move to `theirs`.
    FastForward(ObjectId),
    /// A clean merge; the new two-parent commit (parents `[ours, theirs]`).
    Merged(ObjectId),
    Conflicts(TreeMerge),
}

fn is_binary(data: &[u8]) -> bool {
    data.contains(&0)
}

/// Merges three flat snapshots.
pub fn merge_files(
    base: &Files,
    ours: &Files,
    theirs: &Files,
    ours_label: &str,
    theirs_label: &str,
) -> TreeMerge {
    let paths: BTreeSet<&Vec<u8>> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut files = Files::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
        let resolved: Option<Vec<u8>> = if o == t || b == t {
            o.cloned()
        } else if b == o {
            t.cloned()
        } else {
            match (b, o, t) {
                (Some(_), None, Some(kept)) | (Some(_), Some(kept), None) => {
                    conflicts.push(PathConflict {
                        path: path.clone(),
                        kind: ConflictKind::ModifyDelete {
                            ours_deleted: o.is_none(),
                        },
                    });
                    Some(kept.clone())
                }
                (b, Some(o), Some(t)) => {
                    if b.is_some_and(|b| is_binary(b)) || is_binary(o) || is_binary(t) {
                        conflicts.push(PathConflict {
                            path: path.clone(),
                            kind: ConflictKind::Binary,
                        });
                        Some(o.clone())
                    } else {
                        let m = merge_text(b.map_or(&[][..], |b| b.as_slice()), o, t);
                        match m.clean() {
                            Some(lines) => Some(lines.concat()),
                            None => {
                                conflicts.push(PathConflict {
                                    path: path.clone(),
                                    kind: if b.is_none() {
                                        ConflictKind::BothAdded
                                    } else {
                                        ConflictKind::Content
                                    },
                                });
                                Some(render_with_markers(&m, ours_label, theirs_label))
                            }
                        }
                    }
                }
                _ => unreachable!("all other cases are covered by the equality checks"),
            }
        };
        if let Some(content) = resolved {
            files.insert(path.clone(), content);
        }
    }
    TreeMerge { files, conflicts }
}

impl Repo {
    /// Merges three stored trees (`base` may be absent for unrelated
    /// histories, meaning an empty ancestor).
    pub fn merge_trees(
        &self,
        base: Option<&ObjectId>,
        ours: &ObjectId,
        theirs: &ObjectId,
        ours_label: &str,
        theirs_label: &str,
    ) -> Result<TreeMerge, RepoError> {
        let base = match base {
            Some(b) => self.read_tree(b)?,
            None => Files::new(),
        };
        Ok(merge_files(
            &base,
            &self.read_tree(ours)?,
            &self.read_tree(theirs)?,
            ours_label,
            theirs_label,
        ))
    }

    /// Merges commit `theirs` into commit `ours`.
    pub fn merge_commits(
        &mut self,
        ours: &ObjectId,
        theirs: &ObjectId,
        author: &str,
        message: &str,
        timestamp: u64,
    ) -> Result<MergeOutcome, RepoError> {
        if ours == theirs || self.is_ancestor(theirs, ours)? {
            return Ok(MergeOutcome::AlreadyUpToDate);
        }
        if self.is_ancestor(ours, theirs)? {
            return Ok(MergeOutcome::FastForward(*theirs));
        }
        let base_tree = match self.merge_base(ours, theirs)? {
            Some(b) => Some(self.commit_of(&b)?.tree),
            None => None,
        };
        let tm = self.merge_trees(
            base_tree.as_ref(),
            &self.commit_of(ours)?.tree,
            &self.commit_of(theirs)?.tree,
            "ours",
            "theirs",
        )?;
        if !tm.conflicts.is_empty() {
            return Ok(MergeOutcome::Conflicts(tm));
        }
        let tree = self.write_tree(&tm.files)?;
        let id = self.write_commit(tree, vec![*ours, *theirs], author, message, timestamp)?;
        Ok(MergeOutcome::Merged(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(items: &[(&str, &str)]) -> Files {
        items
            .iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    fn merge(b: &[(&str, &str)], o: &[(&str, &str)], t: &[(&str, &str)]) -> TreeMerge {
        merge_files(&f(b), &f(o), &f(t), "ours", "theirs")
    }

    #[test]
    fn independent_changes_in_different_files_combine() {
        let m = merge(
            &[("a", "1\n"), ("b", "2\n")],
            &[("a", "ONE\n"), ("b", "2\n")],
            &[("a", "1\n"), ("b", "TWO\n"), ("c", "new\n")],
        );
        assert!(m.conflicts.is_empty());
        assert_eq!(
            m.files,
            f(&[("a", "ONE\n"), ("b", "TWO\n"), ("c", "new\n")])
        );
    }

    #[test]
    fn deletions_apply_when_the_other_side_did_not_touch_the_file() {
        let m = merge(
            &[("a", "1\n"), ("b", "2\n")],
            &[("b", "2\n")],
            &[("a", "1\n"), ("b", "2\n")],
        );
        assert!(m.conflicts.is_empty());
        assert_eq!(m.files, f(&[("b", "2\n")]));
        let both = merge(&[("a", "1\n")], &[], &[]);
        assert!(both.files.is_empty() && both.conflicts.is_empty());
    }

    #[test]
    fn the_same_file_edited_in_different_lines_merges_cleanly() {
        let m = merge(
            &[("f", "a\nb\nc\nd\ne\n")],
            &[("f", "A\nb\nc\nd\ne\n")],
            &[("f", "a\nb\nc\nd\nE\n")],
        );
        assert!(m.conflicts.is_empty());
        assert_eq!(m.files, f(&[("f", "A\nb\nc\nd\nE\n")]));
    }

    #[test]
    fn a_content_conflict_is_reported_and_marked() {
        let m = merge(&[("f", "x\n")], &[("f", "ours\n")], &[("f", "theirs\n")]);
        assert_eq!(
            m.conflicts,
            vec![PathConflict {
                path: b"f".to_vec(),
                kind: ConflictKind::Content
            }]
        );
        assert_eq!(
            m.files[&b"f".to_vec()],
            b"<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n".to_vec()
        );
    }

    #[test]
    fn both_adding_the_same_path_differently_is_a_conflict_identically_is_not() {
        let c = merge(&[], &[("n", "x\n")], &[("n", "y\n")]);
        assert_eq!(c.conflicts[0].kind, ConflictKind::BothAdded);
        assert!(merge(&[], &[("n", "x\n")], &[("n", "x\n")])
            .conflicts
            .is_empty());
    }

    #[test]
    fn modify_versus_delete_conflicts_and_keeps_the_modified_file() {
        let m = merge(&[("f", "1\n")], &[], &[("f", "2\n")]);
        assert_eq!(
            m.conflicts[0].kind,
            ConflictKind::ModifyDelete { ours_deleted: true }
        );
        assert_eq!(m.files, f(&[("f", "2\n")]));
        let m = merge(&[("f", "1\n")], &[("f", "2\n")], &[]);
        assert_eq!(
            m.conflicts[0].kind,
            ConflictKind::ModifyDelete {
                ours_deleted: false
            }
        );
    }

    #[test]
    fn binary_files_changed_on_both_sides_conflict_without_markers() {
        let mut b = Files::new();
        b.insert(b"img".to_vec(), vec![0, 1, 2]);
        let (mut o, mut t) = (b.clone(), b.clone());
        o.insert(b"img".to_vec(), vec![0, 9]);
        t.insert(b"img".to_vec(), vec![0, 8]);
        let m = merge_files(&b, &o, &t, "o", "t");
        assert_eq!(m.conflicts[0].kind, ConflictKind::Binary);
        assert_eq!(
            m.files[&b"img".to_vec()],
            vec![0, 9],
            "keeps ours, no markers injected"
        );
    }

    #[test]
    fn commit_level_outcomes() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let commit = |r: &mut Repo, files: &[(&str, &str)], parents: &[ObjectId], ts: u64| {
            let t = r.write_tree(&f(files)).unwrap();
            r.write_commit(t, parents.to_vec(), "a", &format!("c{ts}"), ts)
                .unwrap()
        };
        let base = commit(&mut r, &[("a", "1\n"), ("b", "1\n")], &[], 1);
        let ours = commit(&mut r, &[("a", "OURS\n"), ("b", "1\n")], &[base], 2);
        let theirs = commit(&mut r, &[("a", "1\n"), ("b", "THEIRS\n")], &[base], 3);
        let clash = commit(&mut r, &[("a", "CLASH\n"), ("b", "1\n")], &[base], 4);

        assert_eq!(
            r.merge_commits(&ours, &ours, "m", "x", 9).unwrap(),
            MergeOutcome::AlreadyUpToDate
        );
        assert_eq!(
            r.merge_commits(&ours, &base, "m", "x", 9).unwrap(),
            MergeOutcome::AlreadyUpToDate
        );
        assert_eq!(
            r.merge_commits(&base, &ours, "m", "x", 9).unwrap(),
            MergeOutcome::FastForward(ours)
        );

        let MergeOutcome::Merged(m) = r.merge_commits(&ours, &theirs, "m", "merge", 9).unwrap()
        else {
            panic!("expected a clean merge");
        };
        let mc = r.commit_of(&m).unwrap();
        assert_eq!(mc.parents, vec![ours, theirs]);
        assert_eq!(
            r.read_tree(&mc.tree).unwrap(),
            f(&[("a", "OURS\n"), ("b", "THEIRS\n")])
        );

        let MergeOutcome::Conflicts(tm) = r.merge_commits(&ours, &clash, "m", "x", 9).unwrap()
        else {
            panic!("expected a conflict");
        };
        assert_eq!(tm.conflicts.len(), 1);
        assert_eq!(tm.conflicts[0].path, b"a");
    }

    #[test]
    fn unrelated_histories_merge_against_an_empty_base() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let t1 = r.write_tree(&f(&[("x", "1\n")])).unwrap();
        let t2 = r.write_tree(&f(&[("y", "2\n")])).unwrap();
        let a = r.write_commit(t1, vec![], "a", "a", 1).unwrap();
        let b = r.write_commit(t2, vec![], "a", "b", 2).unwrap();
        let MergeOutcome::Merged(m) = r.merge_commits(&a, &b, "m", "join", 3).unwrap() else {
            panic!()
        };
        assert_eq!(
            r.read_tree(&r.commit_of(&m).unwrap().tree).unwrap(),
            f(&[("x", "1\n"), ("y", "2\n")])
        );
    }
}
