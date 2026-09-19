//! Diffs computed on demand (ticket 004): Myers' shortest edit script over
//! any comparable elements, line-level text diffs, and unified-format
//! output. See `docs/design/decisions/ADR-004-diff.md`.

mod merge;
mod myers;
mod text;

pub use merge::{merge3, merge_text, render_with_markers, Chunk, Kind, Merge3};
pub use myers::{diff, Patch, PatchError, PatchOp};
pub use text::{apply_text, diff_text, split_lines, unified};
