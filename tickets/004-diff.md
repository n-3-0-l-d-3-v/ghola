---
status: done
phase: 8
---

# 004 — Diff computed on demand

## Scope
- Myers O(ND) line diff; edit-script apply round-trips; minimality checked against an O(nm) DP reference.
- Tree diff (added, removed, modified paths) between two commits.

## Done
- [x] Myers O(ND) diff with self-contained patches, invert, apply
- [x] Line diffs and unified output; tree diff that skips unchanged subtrees unread
- [x] Minimality vs LCS DP reference; laziness proven by deleting the subtree; mutants caught
- [x] ADR-004
