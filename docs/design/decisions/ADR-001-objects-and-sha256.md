# ADR-001: Content-addressed objects with one canonical encoding, and SHA-256 from scratch

## Status
Accepted

## Decisions
- **An object's id is the SHA-256 of exactly its stored bytes**, and the stored bytes are `kind byte || payload`. Anything that keeps an object under its id can verify it by hashing what it read, with no side table of lengths or headers (ticket 002 relies on this). The kind byte is inside the hash, so a blob and a tree with identical payload bytes get different ids (tested).
- **Canonical encoding, enforced on decode.** Tree entries are sorted by name bytes; `Tree::new` sorts whatever it is given, and the decoder *rejects* an unsorted or duplicate-named tree instead of normalizing it. Accepting two spellings of one tree would give one tree two ids, breaking the whole premise of content addressing. Property-tested: `decode(bytes)` succeeding implies `encode(decode(bytes)) == bytes`.
- **Names are restricted**: non-empty, not `.` or `..`, no `/`, no NUL. This prevents path-traversal-shaped entries from ever existing in a tree.
- **Timestamps are input, not read.** A commit carries a caller-supplied `u64`; nothing in this repo reads a clock, which keeps commits reproducible and fits the ecosystem's no-wall-clock stance.
- **Parent order is significant** (first parent is the mainline), so reordering parents changes the id (tested).
- **SHA-256 from scratch** (FIPS 180-4), consistent with the ecosystem's habit of building the primitive (CRC-32C, SplitMix64). It is verified against the NIST vectors including the million-`a` input, against the `sha2` crate for every length 0-200 (padding edges) and for arbitrary inputs, and for independence from how input is chunked into `update` calls. `sha2` is a dev-dependency only.
- **Allocation bounds.** A tree declaring four billion entries cannot force a huge allocation: the count is checked against the bytes actually present.

## Testing
12 unit and 7 property tests (512 cases each). Mutation-checked: changing one SHA-256 rotation constant fails two tests; removing the unsorted-tree rejection fails the canonicity test.

## Honest scope note
SHA-256 here is a correct implementation, not a hardened one: it is not constant-time and has had no external audit. That is acceptable for content addressing of non-secret data; it would matter for anything keyed on the digest of secrets.

## Research question (content addressing and history)
Even at this layer the answer is visible: because ids are derived purely from content, identical files across commits are the same object with no work done to deduplicate (measured in ticket 002).
