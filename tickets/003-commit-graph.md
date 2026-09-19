---
status: done
phase: 8
---

# 003 — Commit graph: log, ancestry, merge base

## Scope
- History as a DAG: first-parent and full log order, ancestry test, merge base (lowest common ancestor).
- Property tests on random DAGs against brute-force references.

## Done
- [x] write_commit, ancestors, is_ancestor, topological log, first-parent chain, merge bases
- [x] Property tests vs brute force on random DAGs; two mutants caught
- [x] ADR-003
