---
status: done
phase: 8
---

# 006 — Working tree and the ghola CLI

## Scope
- Snapshot a directory into a tree, check a tree out to a directory, and a real `ghola` CLI: init, commit, log, diff, branch, checkout, merge.
- End-to-end tests of the real binary.

## Done
- [x] `worktree`: directory <-> snapshot, path-traversal guards, untracked-file safety
- [x] `ghola` CLI: init, commit, log, status, diff, branch, checkout, merge (conflict flow, abort)
- [x] Property tests on real directories; 9 end-to-end tests of the real binary; mutants caught
- [x] ADR-006
