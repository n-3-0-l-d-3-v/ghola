//! The sync protocol's byte layouts. Readers return `None` on any malformed
//! input; nothing here can panic on bytes from a peer.

use object::ObjectId;

pub const LIST_REFS: u8 = 0;
pub const FETCH: u8 = 1;
pub const PUT_OBJECTS: u8 = 2;
pub const UPDATE_REF: u8 = 3;

/// Most encoded object bytes in one FETCH response or PUT_OBJECTS request
/// (a single larger object is sent alone).
pub const BATCH_BYTES: usize = 4096;

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn id(&mut self, v: &ObjectId) -> &mut Self {
        self.0.extend(v.0);
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend(v);
        self
    }
    pub fn string(&mut self, v: &str) -> &mut Self {
        self.u16(v.len() as u16);
        self.0.extend(v.as_bytes());
        self
    }
}

pub struct R<'a>(pub &'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn id(&mut self) -> Option<ObjectId> {
        Some(ObjectId(self.take(32)?.try_into().ok()?))
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn string(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    /// A count that is bounded by the bytes remaining, given each item
    /// needs at least `min_item` bytes: a huge declared count cannot force
    /// a huge allocation.
    pub fn count(&mut self, min_item: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        (n <= self.0.len() / min_item.max(1)).then_some(n)
    }
    pub fn finished(&self) -> bool {
        self.0.is_empty()
    }
}
