---
status: done
phase: 8
---

# 002 — Object store and refs on sietch

## Scope
- `crates/repo`: store objects on `sietch` keyed by id, verifying integrity on every read (a flipped bit is detected, never returned).
- Idempotent writes (same content, one copy); branch refs and HEAD; measured deduplication across commits.

## Done
- [x] `crates/repo`: verified object store, idempotent writes, refs, HEAD, snapshot trees
- [x] Corruption detected on read; property tests; two mutants killed (one needed a stronger test)
- [x] Measured deduplication: 3.7% of naive over 100 commits
- [x] ADR-002
