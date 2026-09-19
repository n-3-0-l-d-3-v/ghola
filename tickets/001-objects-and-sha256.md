---
status: open
phase: 8
---

# 001 — Content-addressed objects and a from-scratch SHA-256

## Scope
- `crates/object`: SHA-256 implemented from scratch, checked against NIST vectors and differentially against the `sha2` crate.
- Blob, tree and commit objects with one canonical byte encoding; an object's id is the SHA-256 of exactly its stored bytes.
- Decoding rejects non-canonical input (unsorted or duplicate tree entries, bad names), so one logical object has one id. Decoding arbitrary bytes never panics.
