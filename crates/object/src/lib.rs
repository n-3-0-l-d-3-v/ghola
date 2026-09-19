//! Content-addressed objects and a from-scratch SHA-256 (ticket 001). See
//! `docs/design/decisions/ADR-001-objects-and-sha256.md`.

mod object;
mod sha256;

pub use object::{Commit, EntryKind, Object, ObjectError, ObjectId, Tree, TreeEntry};
pub use sha256::{sha256, Sha256};
