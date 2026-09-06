# impossible-history — THE HISTORY

> A Git-like content-addressed version control system with no stored diffs.

Part of **[The Impossible Computer](https://github.com/n-3-0-l-d-3-v/impossible-computer)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 8 — QUEUED**

See [tickets/](tickets/) for the live phase-by-phase ticket board and
[docs/design/](docs/design/) for constraints, invariants and architecture
decision records.

## The constraint

Every object is immutable and content-addressed. History forms a DAG. Diffs are computed on demand, never stored as the primary historical representation.

## What the constraint forces

Content-addressed object storage on top of impossible-vault, DAG traversal, and network synchronization over impossible-wire.

## Research question

> How naturally does content-addressed immutable storage support version history without storing diffs?

## Sibling repositories

- [impossible-machine](https://github.com/n-3-0-l-d-3-v/impossible-machine) — THE MACHINE (ACTIVE)
- [impossible-language](https://github.com/n-3-0-l-d-3-v/impossible-language) — THE LANGUAGE (QUEUED)
- [impossible-kernel](https://github.com/n-3-0-l-d-3-v/impossible-kernel) — THE KERNEL (QUEUED)
- [impossible-vault](https://github.com/n-3-0-l-d-3-v/impossible-vault) — THE VAULT (QUEUED)
- [impossible-database](https://github.com/n-3-0-l-d-3-v/impossible-database) — THE DATABASE (QUEUED)
- [impossible-wire](https://github.com/n-3-0-l-d-3-v/impossible-wire) — THE WIRE (QUEUED)
- [impossible-colony](https://github.com/n-3-0-l-d-3-v/impossible-colony) — THE COLONY (QUEUED)
- [impossible-artifact](https://github.com/n-3-0-l-d-3-v/impossible-artifact) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
