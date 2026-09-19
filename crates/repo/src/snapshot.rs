//! Snapshots: a flat map of file paths to contents, written to (and read
//! back from) nested tree objects. A snapshot's tree id depends only on its
//! contents, so identical files and identical subdirectories anywhere, in
//! any commit, are the same stored object: deduplication is a consequence of
//! content addressing, not a feature that runs.

use std::collections::BTreeMap;

use object::{EntryKind, Object, ObjectId, Tree, TreeEntry};

use crate::{Repo, RepoError};

/// Path (bytes, `/`-separated, no leading slash) -> file contents.
pub type Files = BTreeMap<Vec<u8>, Vec<u8>>;

type Item<'a> = (&'a [u8], &'a Vec<u8>);

impl Repo {
    /// Writes every blob and tree needed for `files` and returns the root
    /// tree id. Invalid paths (empty components, `.`/`..`, a name used as
    /// both a file and a directory) are errors.
    pub fn write_tree(&mut self, files: &Files) -> Result<ObjectId, RepoError> {
        let items: Vec<Item<'_>> = files.iter().map(|(k, v)| (k.as_slice(), v)).collect();
        self.write_level(&items)
    }

    fn write_level(&mut self, items: &[Item<'_>]) -> Result<ObjectId, RepoError> {
        let mut entries = Vec::new();
        let mut dirs: BTreeMap<&[u8], Vec<Item<'_>>> = BTreeMap::new();
        for (path, content) in items {
            match path.iter().position(|&b| b == b'/') {
                None => {
                    let id = self.put(&Object::Blob((*content).clone()))?;
                    entries.push(TreeEntry {
                        name: path.to_vec(),
                        kind: EntryKind::File,
                        id,
                    });
                }
                Some(i) => dirs
                    .entry(&path[..i])
                    .or_default()
                    .push((&path[i + 1..], content)),
            }
        }
        for (name, sub) in dirs {
            let id = self.write_level(&sub)?;
            entries.push(TreeEntry {
                name: name.to_vec(),
                kind: EntryKind::Dir,
                id,
            });
        }
        self.put(&Object::Tree(Tree::new(entries)?))
    }

    /// The full file listing under a tree.
    pub fn read_tree(&self, id: &ObjectId) -> Result<Files, RepoError> {
        let mut out = Files::new();
        self.read_level(id, &mut Vec::new(), &mut out)?;
        Ok(out)
    }

    fn read_level(
        &self,
        id: &ObjectId,
        prefix: &mut Vec<u8>,
        out: &mut Files,
    ) -> Result<(), RepoError> {
        let Object::Tree(t) = self.require(id)? else {
            return Err(RepoError::WrongType(*id, "tree"));
        };
        for e in t.entries() {
            let mark = prefix.len();
            prefix.extend(&e.name);
            match e.kind {
                EntryKind::File => {
                    let Object::Blob(data) = self.require(&e.id)? else {
                        return Err(RepoError::WrongType(e.id, "blob"));
                    };
                    out.insert(prefix.clone(), data);
                }
                EntryKind::Dir => {
                    prefix.push(b'/');
                    self.read_level(&e.id, prefix, out)?;
                }
            }
            prefix.truncate(mark);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Repo) {
        let d = tempfile::tempdir().unwrap();
        let r = Repo::open(d.path()).unwrap();
        (d, r)
    }

    fn files(items: &[(&str, &str)]) -> Files {
        items
            .iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn nested_files_round_trip() {
        let (_d, mut r) = repo();
        let f = files(&[
            ("a.txt", "1"),
            ("src/main.rs", "fn main(){}"),
            ("src/lib/x.rs", "x"),
            ("z", ""),
        ]);
        let id = r.write_tree(&f).unwrap();
        assert_eq!(r.read_tree(&id).unwrap(), f);
    }

    #[test]
    fn the_same_contents_always_give_the_same_tree_id() {
        let (_d, mut r) = repo();
        let a = r.write_tree(&files(&[("x", "1"), ("d/y", "2")])).unwrap();
        let before = r.stats().objects;
        let b = r.write_tree(&files(&[("d/y", "2"), ("x", "1")])).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            r.stats().objects,
            before,
            "writing an identical snapshot stores nothing new"
        );
    }

    #[test]
    fn identical_files_and_directories_are_stored_once() {
        let (_d, mut r) = repo();
        r.write_tree(&files(&[
            ("one/same.txt", "shared"),
            ("two/same.txt", "shared"),
        ]))
        .unwrap();
        // 1 blob + 1 subtree (shared by both dirs) + 1 root tree.
        assert_eq!(r.stats().objects, 3);
    }

    #[test]
    fn an_empty_snapshot_is_the_empty_tree() {
        let (_d, mut r) = repo();
        let id = r.write_tree(&Files::new()).unwrap();
        assert_eq!(r.read_tree(&id).unwrap(), Files::new());
    }

    #[test]
    fn invalid_paths_are_rejected() {
        let (_d, mut r) = repo();
        for bad in ["", "/a", "a/", "a//b", "./a", "a/../b"] {
            assert!(r.write_tree(&files(&[(bad, "x")])).is_err(), "{bad:?}");
        }
        assert!(
            r.write_tree(&files(&[("a", "file"), ("a/b", "under a file")]))
                .is_err(),
            "a name cannot be both a file and a directory"
        );
    }

    #[test]
    fn reading_a_tree_with_a_missing_blob_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let id = {
            let mut r = Repo::open(d.path()).unwrap();
            let id = r.write_tree(&files(&[("f", "content")])).unwrap();
            // Remove the blob behind the repo's back.
            drop(r);
            let mut raw = Store::open(d.path()).unwrap();
            raw.delete(crate::obj_key(&ObjectId::of(
                &Object::Blob(b"content".to_vec()).encode().unwrap(),
            )))
            .unwrap();
            id
        };
        let r = Repo::open(d.path()).unwrap();
        assert!(matches!(r.read_tree(&id), Err(RepoError::Missing(_))));
    }

    use storage::Store;
}
