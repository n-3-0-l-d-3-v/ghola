---
status: done
phase: 8
---

# 005 — Three-way merge

## Scope
- Line-level three-way merge with explicit conflicts; tree-level merge from a merge base.
- Algebraic properties (merging identical sides is the identity, one-sided changes apply cleanly, symmetry where it should hold).

## Done
- [x] Line-level three-way merge as a lossless alignment; conservative conflicts
- [x] Snapshot and commit merge: typed conflicts, fast-forward, up-to-date, clean merge commit
- [x] Property tests (constructed disjoint/colliding edits, symmetry, soundness); four mutants caught
- [x] ADR-005
