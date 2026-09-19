//! Immutable, content-addressed objects: blob, tree, commit.
//!
//! An object's bytes are `kind byte || payload`, and its id is the SHA-256
//! of exactly those bytes, so anything that stores an object under its id
//! can verify it by hashing what it read. Encoding is canonical: one
//! logical object has exactly one byte string and therefore one id. Tree
//! entries are kept sorted by name; a decoder rejects an unsorted or
//! duplicated tree rather than normalizing it (accepting two encodings of
//! one tree would give one tree two ids).
//!
//! ```text
//! blob:   0x01, content
//! tree:   0x02, count u32 LE, count x (kind u8, name_len u16 LE, name, id[32])
//!         kind: 0 = file, 1 = directory; names strictly ascending by bytes
//! commit: 0x03, tree id[32], parent_count u8, parents x id[32],
//!         timestamp u64 LE, author_len u16 LE, author, message_len u32 LE, message
//! ```
//! The timestamp is supplied by the caller: nothing in this crate reads a
//! clock.

use std::fmt;

use crate::sha256::sha256;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    pub fn of(bytes: &[u8]) -> ObjectId {
        ObjectId(sha256(bytes))
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<ObjectId> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(ObjectId(out))
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", &self.to_hex()[..12])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub id: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    pub timestamp: u64,
    pub author: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Tree),
    Commit(Commit),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ObjectError {
    #[error("empty object")]
    Empty,
    #[error("unknown object kind byte {0}")]
    UnknownKind(u8),
    #[error("object data truncated")]
    Truncated,
    #[error("object has trailing bytes")]
    TrailingBytes,
    #[error("invalid tree entry name {0:?}")]
    InvalidName(Vec<u8>),
    #[error("duplicate tree entry name {0:?}")]
    DuplicateName(Vec<u8>),
    #[error("tree entries are not in canonical (ascending) order")]
    UnsortedTree,
    #[error("unknown tree entry kind {0}")]
    UnknownEntryKind(u8),
    #[error("commit text is not valid UTF-8")]
    InvalidUtf8,
    #[error("a commit can have at most 255 parents")]
    TooManyParents,
    #[error("field too long to encode")]
    TooLong,
}

fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

impl Tree {
    /// Builds a tree from entries in any order; the result is canonical
    /// (sorted). Rejects invalid or duplicate names.
    pub fn new(mut entries: Vec<TreeEntry>) -> Result<Tree, ObjectError> {
        for e in &entries {
            if !valid_name(&e.name) {
                return Err(ObjectError::InvalidName(e.name.clone()));
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(w) = entries.windows(2).find(|w| w[0].name == w[1].name) {
            return Err(ObjectError::DuplicateName(w[0].name.clone()));
        }
        Ok(Tree { entries })
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn get(&self, name: &[u8]) -> Option<&TreeEntry> {
        self.entries
            .binary_search_by(|e| e.name.as_slice().cmp(name))
            .ok()
            .map(|i| &self.entries[i])
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ObjectError> {
        if self.0.len() < n {
            return Err(ObjectError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, ObjectError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ObjectError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, ObjectError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ObjectError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn id(&mut self) -> Result<ObjectId, ObjectError> {
        Ok(ObjectId(self.take(32)?.try_into().unwrap()))
    }
}

impl Object {
    /// The canonical stored bytes: `kind || payload`.
    pub fn encode(&self) -> Result<Vec<u8>, ObjectError> {
        let mut out = Vec::new();
        match self {
            Object::Blob(data) => {
                out.push(1);
                out.extend(data);
            }
            Object::Tree(t) => {
                out.push(2);
                out.extend((t.entries.len() as u32).to_le_bytes());
                for e in &t.entries {
                    out.push(match e.kind {
                        EntryKind::File => 0,
                        EntryKind::Dir => 1,
                    });
                    let len = u16::try_from(e.name.len()).map_err(|_| ObjectError::TooLong)?;
                    out.extend(len.to_le_bytes());
                    out.extend(&e.name);
                    out.extend(e.id.0);
                }
            }
            Object::Commit(c) => {
                out.push(3);
                out.extend(c.tree.0);
                let n = u8::try_from(c.parents.len()).map_err(|_| ObjectError::TooManyParents)?;
                out.push(n);
                for p in &c.parents {
                    out.extend(p.0);
                }
                out.extend(c.timestamp.to_le_bytes());
                let alen = u16::try_from(c.author.len()).map_err(|_| ObjectError::TooLong)?;
                out.extend(alen.to_le_bytes());
                out.extend(c.author.as_bytes());
                let mlen = u32::try_from(c.message.len()).map_err(|_| ObjectError::TooLong)?;
                out.extend(mlen.to_le_bytes());
                out.extend(c.message.as_bytes());
            }
        }
        Ok(out)
    }

    pub fn id(&self) -> Result<ObjectId, ObjectError> {
        Ok(ObjectId::of(&self.encode()?))
    }

    /// Decodes canonical bytes. Never panics; rejects anything that
    /// `encode` could not have produced.
    pub fn decode(bytes: &[u8]) -> Result<Object, ObjectError> {
        let (&kind, payload) = bytes.split_first().ok_or(ObjectError::Empty)?;
        match kind {
            1 => Ok(Object::Blob(payload.to_vec())),
            2 => {
                let mut r = Reader(payload);
                let count = r.u32()? as usize;
                // Each entry is at least 35 bytes, bounding the allocation.
                if count > r.0.len() / 35 {
                    return Err(ObjectError::Truncated);
                }
                let mut entries: Vec<TreeEntry> = Vec::with_capacity(count);
                for _ in 0..count {
                    let kind = match r.u8()? {
                        0 => EntryKind::File,
                        1 => EntryKind::Dir,
                        other => return Err(ObjectError::UnknownEntryKind(other)),
                    };
                    let len = r.u16()? as usize;
                    let name = r.take(len)?.to_vec();
                    if !valid_name(&name) {
                        return Err(ObjectError::InvalidName(name));
                    }
                    let id = r.id()?;
                    if let Some(prev) = entries.last() {
                        match prev.name.cmp(&name) {
                            std::cmp::Ordering::Less => {}
                            std::cmp::Ordering::Equal => {
                                return Err(ObjectError::DuplicateName(name))
                            }
                            std::cmp::Ordering::Greater => return Err(ObjectError::UnsortedTree),
                        }
                    }
                    entries.push(TreeEntry { name, kind, id });
                }
                if !r.0.is_empty() {
                    return Err(ObjectError::TrailingBytes);
                }
                Ok(Object::Tree(Tree { entries }))
            }
            3 => {
                let mut r = Reader(payload);
                let tree = r.id()?;
                let n = r.u8()? as usize;
                let mut parents = Vec::with_capacity(n);
                for _ in 0..n {
                    parents.push(r.id()?);
                }
                let timestamp = r.u64()?;
                let alen = r.u16()? as usize;
                let author = String::from_utf8(r.take(alen)?.to_vec())
                    .map_err(|_| ObjectError::InvalidUtf8)?;
                let mlen = r.u32()? as usize;
                let message = String::from_utf8(r.take(mlen)?.to_vec())
                    .map_err(|_| ObjectError::InvalidUtf8)?;
                if !r.0.is_empty() {
                    return Err(ObjectError::TrailingBytes);
                }
                Ok(Object::Commit(Commit {
                    tree,
                    parents,
                    timestamp,
                    author,
                    message,
                }))
            }
            other => Err(ObjectError::UnknownKind(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ObjectId {
        ObjectId([n; 32])
    }

    fn entry(name: &str, kind: EntryKind, n: u8) -> TreeEntry {
        TreeEntry {
            name: name.into(),
            kind,
            id: id(n),
        }
    }

    #[test]
    fn identical_content_has_identical_ids_and_different_content_does_not() {
        let a = Object::Blob(b"hello".to_vec());
        assert_eq!(
            a.id().unwrap(),
            Object::Blob(b"hello".to_vec()).id().unwrap()
        );
        assert_ne!(
            a.id().unwrap(),
            Object::Blob(b"hellp".to_vec()).id().unwrap()
        );
    }

    #[test]
    fn a_blob_and_a_tree_with_the_same_payload_bytes_get_different_ids() {
        // The kind byte is part of what is hashed.
        let empty_blob = Object::Blob(vec![]).id().unwrap();
        let empty_tree = Object::Tree(Tree::default()).id().unwrap();
        assert_ne!(empty_blob, empty_tree);
    }

    #[test]
    fn a_stored_object_verifies_by_hashing_its_own_bytes() {
        let o = Object::Blob(b"data".to_vec());
        let bytes = o.encode().unwrap();
        assert_eq!(ObjectId::of(&bytes), o.id().unwrap());
    }

    #[test]
    fn tree_construction_is_order_independent() {
        let a = Tree::new(vec![
            entry("b", EntryKind::File, 1),
            entry("a", EntryKind::Dir, 2),
            entry("c", EntryKind::File, 3),
        ])
        .unwrap();
        let b = Tree::new(vec![
            entry("c", EntryKind::File, 3),
            entry("a", EntryKind::Dir, 2),
            entry("b", EntryKind::File, 1),
        ])
        .unwrap();
        assert_eq!(
            Object::Tree(a.clone()).id().unwrap(),
            Object::Tree(b).id().unwrap()
        );
        assert_eq!(a.get(b"a").unwrap().kind, EntryKind::Dir);
        assert!(a.get(b"zzz").is_none());
    }

    #[test]
    fn invalid_and_duplicate_names_are_rejected() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert!(
                matches!(
                    Tree::new(vec![entry(bad, EntryKind::File, 1)]),
                    Err(ObjectError::InvalidName(_))
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            Tree::new(vec![
                entry("a", EntryKind::File, 1),
                entry("a", EntryKind::Dir, 2)
            ]),
            Err(ObjectError::DuplicateName(_))
        ));
    }

    #[test]
    fn a_non_canonical_tree_encoding_is_rejected_not_normalized() {
        let t = Tree::new(vec![
            entry("a", EntryKind::File, 1),
            entry("b", EntryKind::File, 2),
        ])
        .unwrap();
        let mut bytes = Object::Tree(t).encode().unwrap();
        // Swap the two 36-byte entries (kind + len(2) + name(1) + id(32) = 36).
        let (head, body) = bytes.split_at_mut(5);
        let (x, y) = body.split_at_mut(36);
        let _ = head;
        x.swap_with_slice(y);
        assert_eq!(Object::decode(&bytes), Err(ObjectError::UnsortedTree));
    }

    #[test]
    fn commits_round_trip_and_parent_order_matters() {
        let c = Commit {
            tree: id(1),
            parents: vec![id(2), id(3)],
            timestamp: 1_700_000_000,
            author: "Ada <ada@example.com>".into(),
            message: "first\n\nbody ünïcode".into(),
        };
        let o = Object::Commit(c.clone());
        assert_eq!(Object::decode(&o.encode().unwrap()), Ok(o.clone()));
        let mut swapped = c;
        swapped.parents.reverse();
        assert_ne!(Object::Commit(swapped).id().unwrap(), o.id().unwrap());
    }

    #[test]
    fn malformed_input_is_a_typed_error() {
        assert_eq!(Object::decode(&[]), Err(ObjectError::Empty));
        assert_eq!(Object::decode(&[9]), Err(ObjectError::UnknownKind(9)));
        assert_eq!(
            Object::decode(&[2, 1, 0, 0, 0]),
            Err(ObjectError::Truncated)
        );
        assert_eq!(Object::decode(&[3, 0]), Err(ObjectError::Truncated));
        let mut ok = Object::Tree(Tree::default()).encode().unwrap();
        ok.push(0);
        assert_eq!(Object::decode(&ok), Err(ObjectError::TrailingBytes));
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        let i = ObjectId::of(b"x");
        assert_eq!(ObjectId::from_hex(&i.to_hex()), Some(i));
        assert_eq!(ObjectId::from_hex("zz"), None);
        assert_eq!(ObjectId::from_hex(&"g".repeat(64)), None);
        assert_eq!(ObjectId::from_hex(&"é".repeat(32)), None);
    }

    #[test]
    fn a_huge_declared_tree_count_cannot_force_a_huge_allocation() {
        let mut b = vec![2];
        b.extend(u32::MAX.to_le_bytes());
        assert_eq!(Object::decode(&b), Err(ObjectError::Truncated));
    }
}
