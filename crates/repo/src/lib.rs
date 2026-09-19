//! The repository: an immutable object store, refs and HEAD, all on one
//! `sietch::Store` (ticket 002). See `docs/design/decisions/ADR-002-*.md`.
//!
//! Objects are stored under `o` + id and never modified. `get` hashes what
//! it read and compares it to the id it was asked for, so corruption (a
//! flipped bit on disk, a tampered value) is reported, never returned as
//! data. Refs (`r/<name>` -> id) and HEAD are the only mutable state; on an
//! append-only store even those are new versions, not overwrites.

mod snapshot;

use std::collections::BTreeMap;
use std::path::PathBuf;

use object::{Commit, Object, ObjectError, ObjectId};
use storage::{Store, StoreError};

pub use snapshot::Files;

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Object(#[from] ObjectError),
    #[error("object {0} is corrupt: its bytes do not hash to its id")]
    Corrupt(ObjectId),
    #[error("object {0} is missing")]
    Missing(ObjectId),
    #[error("object {0} has the wrong type: expected {1}")]
    WrongType(ObjectId, &'static str),
    #[error("invalid ref name {0:?}")]
    InvalidRefName(String),
    #[error("HEAD data is malformed")]
    BadHead,
}

/// What HEAD points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// A branch name (which may not have any commits yet).
    Branch(String),
    Detached(ObjectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepoStats {
    pub objects: usize,
    pub bytes: usize,
}

pub struct Repo {
    store: Store,
}

const OBJ: u8 = b'o';
const REF_PREFIX: &[u8] = b"r/";
const HEAD_KEY: &[u8] = b"HEAD";

fn obj_key(id: &ObjectId) -> Vec<u8> {
    let mut k = vec![OBJ];
    k.extend(id.0);
    k
}

fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("//")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

impl Repo {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, RepoError> {
        Ok(Repo {
            store: Store::open(dir)?,
        })
    }

    /// Stores an object and returns its id. Storing the same content again
    /// writes nothing.
    pub fn put(&mut self, o: &Object) -> Result<ObjectId, RepoError> {
        let bytes = o.encode()?;
        let id = ObjectId::of(&bytes);
        let key = obj_key(&id);
        if self.store.get(&key).is_none() {
            self.store.put(key, bytes)?;
        }
        Ok(id)
    }

    pub fn has(&self, id: &ObjectId) -> bool {
        self.store.get(&obj_key(id)).is_some()
    }

    /// Reads and verifies an object. `Ok(None)` if it isn't stored.
    pub fn get(&self, id: &ObjectId) -> Result<Option<Object>, RepoError> {
        let Some(bytes) = self.store.get(&obj_key(id)) else {
            return Ok(None);
        };
        if ObjectId::of(&bytes) != *id {
            return Err(RepoError::Corrupt(*id));
        }
        Ok(Some(Object::decode(&bytes)?))
    }

    pub fn require(&self, id: &ObjectId) -> Result<Object, RepoError> {
        self.get(id)?.ok_or(RepoError::Missing(*id))
    }

    pub fn commit_of(&self, id: &ObjectId) -> Result<Commit, RepoError> {
        match self.require(id)? {
            Object::Commit(c) => Ok(c),
            _ => Err(RepoError::WrongType(*id, "commit")),
        }
    }

    /// Every stored object id, in id order.
    pub fn object_ids(&self) -> Vec<ObjectId> {
        self.store
            .scan(&[OBJ])
            .into_iter()
            .filter_map(|(k, _)| Some(ObjectId(k[1..].try_into().ok()?)))
            .collect()
    }

    pub fn stats(&self) -> RepoStats {
        let all = self.store.scan(&[OBJ]);
        RepoStats {
            objects: all.len(),
            bytes: all.iter().map(|(_, v)| v.len()).sum(),
        }
    }

    pub fn set_ref(&mut self, name: &str, id: ObjectId) -> Result<(), RepoError> {
        if !valid_ref_name(name) {
            return Err(RepoError::InvalidRefName(name.to_string()));
        }
        self.store
            .put([REF_PREFIX, name.as_bytes()].concat(), id.0.to_vec())?;
        Ok(())
    }

    pub fn get_ref(&self, name: &str) -> Option<ObjectId> {
        let v = self.store.get(&[REF_PREFIX, name.as_bytes()].concat())?;
        Some(ObjectId(v.try_into().ok()?))
    }

    pub fn delete_ref(&mut self, name: &str) -> Result<(), RepoError> {
        self.store.delete([REF_PREFIX, name.as_bytes()].concat())?;
        Ok(())
    }

    /// All refs, sorted by name.
    pub fn refs(&self) -> BTreeMap<String, ObjectId> {
        self.store
            .scan(REF_PREFIX)
            .into_iter()
            .filter_map(|(k, v)| {
                Some((
                    String::from_utf8(k[REF_PREFIX.len()..].to_vec()).ok()?,
                    ObjectId(v.try_into().ok()?),
                ))
            })
            .collect()
    }

    pub fn set_head(&mut self, head: &Head) -> Result<(), RepoError> {
        let bytes = match head {
            Head::Branch(name) => {
                if !valid_ref_name(name) {
                    return Err(RepoError::InvalidRefName(name.clone()));
                }
                let mut v = vec![0];
                v.extend(name.as_bytes());
                v
            }
            Head::Detached(id) => {
                let mut v = vec![1];
                v.extend(id.0);
                v
            }
        };
        self.store.put(HEAD_KEY.to_vec(), bytes)?;
        Ok(())
    }

    pub fn head(&self) -> Result<Option<Head>, RepoError> {
        let Some(v) = self.store.get(HEAD_KEY) else {
            return Ok(None);
        };
        match v.split_first() {
            Some((0, name)) => Ok(Some(Head::Branch(
                String::from_utf8(name.to_vec()).map_err(|_| RepoError::BadHead)?,
            ))),
            Some((1, id)) => Ok(Some(Head::Detached(ObjectId(
                id.try_into().map_err(|_| RepoError::BadHead)?,
            )))),
            _ => Err(RepoError::BadHead),
        }
    }

    /// The commit HEAD currently resolves to, if any.
    pub fn head_commit(&self) -> Result<Option<ObjectId>, RepoError> {
        Ok(match self.head()? {
            None => None,
            Some(Head::Detached(id)) => Some(id),
            Some(Head::Branch(name)) => self.get_ref(&name),
        })
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

    #[test]
    fn objects_round_trip_and_writes_are_idempotent() {
        let (_d, mut r) = repo();
        let o = Object::Blob(b"hello".to_vec());
        let id = r.put(&o).unwrap();
        assert_eq!(r.put(&o).unwrap(), id);
        assert_eq!(r.stats().objects, 1);
        assert_eq!(r.get(&id).unwrap(), Some(o));
        assert!(r.has(&id));
        assert_eq!(r.get(&ObjectId::of(b"nope")).unwrap(), None);
        assert!(matches!(
            r.require(&ObjectId::of(b"nope")),
            Err(RepoError::Missing(_))
        ));
    }

    #[test]
    fn a_tampered_object_is_reported_corrupt_never_returned() {
        let d = tempfile::tempdir().unwrap();
        let id = {
            let mut r = Repo::open(d.path()).unwrap();
            r.put(&Object::Blob(b"precious".to_vec())).unwrap()
        };
        {
            // Overwrite the stored value behind the repo's back.
            let mut raw = Store::open(d.path()).unwrap();
            let mut bad = Object::Blob(b"precious".to_vec()).encode().unwrap();
            *bad.last_mut().unwrap() ^= 1;
            raw.put(obj_key(&id), bad).unwrap();
        }
        let r = Repo::open(d.path()).unwrap();
        assert!(matches!(r.get(&id), Err(RepoError::Corrupt(c)) if c == id));
    }

    #[test]
    fn commits_are_typed_on_read() {
        let (_d, mut r) = repo();
        let blob = r.put(&Object::Blob(vec![1])).unwrap();
        assert!(matches!(r.commit_of(&blob), Err(RepoError::WrongType(..))));
    }

    #[test]
    fn refs_set_get_list_delete_and_validate() {
        let (_d, mut r) = repo();
        let a = ObjectId::of(b"a");
        r.set_ref("main", a).unwrap();
        r.set_ref("feature/x-1", a).unwrap();
        assert_eq!(r.get_ref("main"), Some(a));
        assert_eq!(
            r.refs().keys().collect::<Vec<_>>(),
            vec!["feature/x-1", "main"]
        );
        r.delete_ref("main").unwrap();
        assert_eq!(r.get_ref("main"), None);
        for bad in ["", "/a", "a/", "a//b", "a..b", "has space", "ünï"] {
            assert!(
                matches!(r.set_ref(bad, a), Err(RepoError::InvalidRefName(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn head_states_and_resolution() {
        let (_d, mut r) = repo();
        assert_eq!(r.head().unwrap(), None);
        r.set_head(&Head::Branch("main".into())).unwrap();
        assert_eq!(r.head_commit().unwrap(), None, "unborn branch");
        let c = ObjectId::of(b"c");
        r.set_ref("main", c).unwrap();
        assert_eq!(r.head_commit().unwrap(), Some(c));
        let d = ObjectId::of(b"d");
        r.set_head(&Head::Detached(d)).unwrap();
        assert_eq!(r.head().unwrap(), Some(Head::Detached(d)));
        assert_eq!(r.head_commit().unwrap(), Some(d));
        assert!(r.set_head(&Head::Branch("bad name".into())).is_err());
    }

    #[test]
    fn everything_survives_a_real_close_and_reopen() {
        let d = tempfile::tempdir().unwrap();
        let (id, c) = {
            let mut r = Repo::open(d.path()).unwrap();
            let id = r.put(&Object::Blob(b"x".to_vec())).unwrap();
            r.set_ref("main", id).unwrap();
            r.set_head(&Head::Branch("main".into())).unwrap();
            (id, ObjectId::of(b"c"))
        };
        let r = Repo::open(d.path()).unwrap();
        assert!(r.has(&id));
        assert_eq!(r.head_commit().unwrap(), Some(id));
        assert_eq!(r.get_ref("nope"), None);
        let _ = c;
        assert_eq!(r.object_ids(), vec![id]);
    }

    fn disk_bytes(dir: &std::path::Path) -> u64 {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum()
    }

    #[test]
    fn rewriting_an_existing_object_appends_nothing_to_disk() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Repo::open(d.path()).unwrap();
        let o = Object::Blob(vec![7; 500]);
        r.put(&o).unwrap();
        let before = disk_bytes(d.path());
        r.put(&o).unwrap();
        r.put(&o).unwrap();
        assert_eq!(
            disk_bytes(d.path()),
            before,
            "an identical put must not append a record"
        );
    }
}
