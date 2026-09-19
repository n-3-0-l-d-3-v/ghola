---
status: open
phase: 8
---

# 002 — Object store and refs on sietch

## Scope
- `crates/repo`: store objects on `sietch` keyed by id, verifying integrity on every read (a flipped bit is detected, never returned).
- Idempotent writes (same content, one copy); branch refs and HEAD; measured deduplication across commits.
