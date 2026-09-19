//! Which files changed between two snapshots. Because a tree id is the hash
//! of its contents, two entries with the same id are identical all the way
//! down, so unchanged subtrees are skipped by comparing ids alone and are
//! never read from the store.

use object::{EntryKind, Object, ObjectId, Tree};

use crate::{Repo, RepoError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added {
        path: Vec<u8>,
        id: ObjectId,
    },
    Removed {
        path: Vec<u8>,
        id: ObjectId,
    },
    Modified {
        path: Vec<u8>,
        old: ObjectId,
        new: ObjectId,
    },
}

impl Change {
    pub fn path(&self) -> &[u8] {
        match self {
            Change::Added { path, .. }
            | Change::Removed { path, .. }
            | Change::Modified { path, .. } => path,
        }
    }
}

impl Repo {
    fn tree_of(&self, id: &ObjectId) -> Result<Tree, RepoError> {
        match self.require(id)? {
            Object::Tree(t) => Ok(t),
            _ => Err(RepoError::WrongType(*id, "tree")),
        }
    }

    /// Every file under a tree, as `(path, blob id)`.
    fn flatten(
        &self,
        tree: &ObjectId,
        prefix: &mut Vec<u8>,
        out: &mut Vec<(Vec<u8>, ObjectId)>,
    ) -> Result<(), RepoError> {
        for e in self.tree_of(tree)?.entries() {
            let mark = prefix.len();
            prefix.extend(&e.name);
            match e.kind {
                EntryKind::File => out.push((prefix.clone(), e.id)),
                EntryKind::Dir => {
                    prefix.push(b'/');
                    self.flatten(&e.id, prefix, out)?;
                }
            }
            prefix.truncate(mark);
        }
        Ok(())
    }

    /// The file-level changes turning tree `a` into tree `b`, sorted by path.
    pub fn diff_trees(&self, a: &ObjectId, b: &ObjectId) -> Result<Vec<Change>, RepoError> {
        let mut out = Vec::new();
        if a != b {
            self.diff_level(a, b, &mut Vec::new(), &mut out)?;
        }
        out.sort_by(|x, y| x.path().cmp(y.path()));
        Ok(out)
    }

    fn diff_level(
        &self,
        a: &ObjectId,
        b: &ObjectId,
        prefix: &mut Vec<u8>,
        out: &mut Vec<Change>,
    ) -> Result<(), RepoError> {
        let (ta, tb) = (self.tree_of(a)?, self.tree_of(b)?);
        let (ea, eb) = (ta.entries(), tb.entries());
        let (mut i, mut j) = (0, 0);
        while i < ea.len() || j < eb.len() {
            let order = match (ea.get(i), eb.get(j)) {
                (Some(x), Some(y)) => x.name.cmp(&y.name),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, _) => std::cmp::Ordering::Greater,
            };
            let mark = prefix.len();
            match order {
                std::cmp::Ordering::Less => {
                    let e = &ea[i];
                    prefix.extend(&e.name);
                    self.emit_all(e.kind, &e.id, prefix, out, true)?;
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    let e = &eb[j];
                    prefix.extend(&e.name);
                    self.emit_all(e.kind, &e.id, prefix, out, false)?;
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    let (x, y) = (&ea[i], &eb[j]);
                    prefix.extend(&x.name);
                    if x.id != y.id {
                        match (x.kind, y.kind) {
                            (EntryKind::File, EntryKind::File) => out.push(Change::Modified {
                                path: prefix.clone(),
                                old: x.id,
                                new: y.id,
                            }),
                            (EntryKind::Dir, EntryKind::Dir) => {
                                prefix.push(b'/');
                                self.diff_level(&x.id, &y.id, prefix, out)?;
                            }
                            _ => {
                                self.emit_all(x.kind, &x.id, prefix, out, true)?;
                                self.emit_all(y.kind, &y.id, prefix, out, false)?;
                            }
                        }
                    }
                    i += 1;
                    j += 1;
                }
            }
            prefix.truncate(mark);
        }
        Ok(())
    }

    /// Emits `Removed` (or `Added`) for a file, or for every file under a directory.
    fn emit_all(
        &self,
        kind: EntryKind,
        id: &ObjectId,
        path: &mut Vec<u8>,
        out: &mut Vec<Change>,
        removed: bool,
    ) -> Result<(), RepoError> {
        let mut files = Vec::new();
        match kind {
            EntryKind::File => files.push((path.clone(), *id)),
            EntryKind::Dir => {
                path.push(b'/');
                self.flatten(id, path, &mut files)?;
                path.pop();
            }
        }
        for (path, id) in files {
            out.push(if removed {
                Change::Removed { path, id }
            } else {
                Change::Added { path, id }
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Files;

    fn files(items: &[(&str, &str)]) -> Files {
        items
            .iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    fn repo() -> (tempfile::TempDir, Repo) {
        let d = tempfile::tempdir().unwrap();
        let r = Repo::open(d.path()).unwrap();
        (d, r)
    }

    fn summary(changes: &[Change]) -> Vec<String> {
        changes
            .iter()
            .map(|c| match c {
                Change::Added { path, .. } => format!("A {}", String::from_utf8_lossy(path)),
                Change::Removed { path, .. } => format!("D {}", String::from_utf8_lossy(path)),
                Change::Modified { path, .. } => format!("M {}", String::from_utf8_lossy(path)),
            })
            .collect()
    }

    #[test]
    fn added_removed_and_modified_files() {
        let (_d, mut r) = repo();
        let a = r
            .write_tree(&files(&[
                ("keep", "1"),
                ("gone", "2"),
                ("d/x", "3"),
                ("d/y", "4"),
            ]))
            .unwrap();
        let b = r
            .write_tree(&files(&[
                ("keep", "1"),
                ("new", "5"),
                ("d/x", "3"),
                ("d/y", "changed"),
            ]))
            .unwrap();
        assert_eq!(
            summary(&r.diff_trees(&a, &b).unwrap()),
            vec!["M d/y", "D gone", "A new"]
        );
        assert!(r.diff_trees(&a, &a).unwrap().is_empty());
    }

    #[test]
    fn whole_directories_appear_and_disappear_file_by_file() {
        let (_d, mut r) = repo();
        let a = r
            .write_tree(&files(&[("f", "1"), ("old/a", "x"), ("old/sub/b", "y")]))
            .unwrap();
        let b = r
            .write_tree(&files(&[("f", "1"), ("fresh/c", "z")]))
            .unwrap();
        assert_eq!(
            summary(&r.diff_trees(&a, &b).unwrap()),
            vec!["A fresh/c", "D old/a", "D old/sub/b"]
        );
    }

    #[test]
    fn a_file_becoming_a_directory_and_back() {
        let (_d, mut r) = repo();
        let a = r.write_tree(&files(&[("x", "file")])).unwrap();
        let b = r.write_tree(&files(&[("x/inner", "now a dir")])).unwrap();
        assert_eq!(
            summary(&r.diff_trees(&a, &b).unwrap()),
            vec!["D x", "A x/inner"]
        );
        assert_eq!(
            summary(&r.diff_trees(&b, &a).unwrap()),
            vec!["A x", "D x/inner"]
        );
    }

    #[test]
    fn a_missing_tree_is_an_error() {
        let (_d, mut r) = repo();
        let a = r.write_tree(&files(&[("f", "1")])).unwrap();
        assert!(matches!(
            r.diff_trees(&a, &ObjectId::of(b"nope")),
            Err(RepoError::Missing(_))
        ));
    }
}
