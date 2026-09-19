---
status: done
phase: 8
---

# 007 — Synchronization over distrans and phase close

## Scope
- Push/pull of the objects the other side lacks over distrans's reliable transport on a hostile channel; convergence under faults.
- Measurements, ADR, phase close.

## Done
- [x] Request protocol, pure server, fetch/push/pull with dangling-ref protection
- [x] `DistransRemote`: distrans RPC + transport + hostile simulated channel
- [x] 13 sync tests + 4 CLI end-to-end tests; four protocol mutants caught
- [x] Measurements; finding on distrans RPC retry behaviour; ADR-007; Phase 8 closed
