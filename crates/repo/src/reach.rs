//! Reachability and raw ingestion, the two things synchronization needs
//! from a repository: "which objects does this commit depend on?" and "store
//! these bytes only if they really are a valid object".

use std::collections::BTreeSet;

use object::{EntryKind, Object, ObjectId};

use crate::{Repo, RepoError};

impl Repo {
    /// Every object reachable from `roots` (the roots themselves, commit
    /// parents and trees, subtrees, blobs). Fails with `Missing` if anything
    /// in the closure is absent, so success means the closure is complete.
    pub fn reachable(&self, roots: &[ObjectId]) -> Result<BTreeSet<ObjectId>, RepoError> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<ObjectId> = roots.to_vec();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match self.require(&id)? {
                Object::Blob(_) => {}
                Object::Tree(t) => {
                    for e in t.entries() {
                        debug_assert!(matches!(e.kind, EntryKind::File | EntryKind::Dir));
                        stack.push(e.id);
                    }
                }
                Object::Commit(c) => {
                    stack.push(c.tree);
                    stack.extend(c.parents);
                }
            }
        }
        Ok(seen)
    }

    /// True if `id` and everything it depends on is present and verifies.
    pub fn is_complete(&self, id: &ObjectId) -> bool {
        self.reachable(&[*id]).is_ok()
    }

    /// Stores the object whose canonical encoding is exactly `bytes`,
    /// returning its id. Anything that is not the canonical encoding of a
    /// valid object is rejected, so a peer cannot plant malformed or
    /// non-canonical data.
    pub fn put_raw(&mut self, bytes: &[u8]) -> Result<ObjectId, RepoError> {
        let obj = Object::decode(bytes)?;
        let id = self.put(&obj)?;
        debug_assert_eq!(id, ObjectId::of(bytes));
        Ok(id)
    }

    /// The canonical bytes of a stored object (verified on the way out).
    pub fn raw(&self, id: &ObjectId) -> Result<Vec<u8>, RepoError> {
        Ok(self.require(id)?.encode()?)
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

    #[test]
    fn reachable_covers_commits_parents_trees_and_blobs() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let t1 = r.write_tree(&files(&[("a", "1"), ("d/b", "2")])).unwrap();
        let c1 = r.write_commit(t1, vec![], "x", "one", 1).unwrap();
        let t2 = r.write_tree(&files(&[("a", "1"), ("d/b", "3")])).unwrap();
        let c2 = r.write_commit(t2, vec![c1], "x", "two", 2).unwrap();
        let all = r.reachable(&[c2]).unwrap();
        assert!(all.contains(&c1) && all.contains(&c2) && all.contains(&t1) && all.contains(&t2));
        assert_eq!(
            all.len(),
            r.stats().objects,
            "everything stored is reachable from c2"
        );
        assert!(r.reachable(&[c1]).unwrap().len() < all.len());
        assert!(r.is_complete(&c2));
    }

    #[test]
    fn a_missing_object_makes_the_closure_incomplete() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let ghost = ObjectId::of(b"ghost");
        let c = r.write_commit(ghost, vec![], "x", "orphan", 1).unwrap();
        assert!(!r.is_complete(&c));
        assert!(matches!(r.reachable(&[c]), Err(RepoError::Missing(_))));
    }

    #[test]
    fn put_raw_accepts_only_canonical_valid_objects() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let bytes = Object::Blob(b"hello".to_vec()).encode().unwrap();
        let id = r.put_raw(&bytes).unwrap();
        assert_eq!(id, ObjectId::of(&bytes));
        assert_eq!(r.raw(&id).unwrap(), bytes);
        for bad in [vec![], vec![9, 9], vec![2, 1, 0, 0, 0]] {
            assert!(r.put_raw(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(r.stats().objects, 1, "rejected input stores nothing");
    }
}
