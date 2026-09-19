# ADR-003: The commit DAG, a timestamp-free topological log, and merge bases as the definition says

## Status
Accepted

## Decisions
- **History is only parent links.** Every query walks `parents` through the verified object store; nothing is cached, so nothing can go stale or disagree with the objects.
- **`log` is a true topological order** (a commit always precedes all its parents) computed by Kahn's algorithm over the reachable subgraph. Independent branches are interleaved newest-timestamp-first, then by id, so the order is deterministic, but the timestamp is *only* a tie-break: a test gives a parent a newer timestamp than its child (clock skew, which real repositories have) and the order is still correct. Ordering by timestamp alone, as naive log implementations do, would get that wrong.
- **Merge bases are the definition**: common ancestors that are not ancestors of another common ancestor. It returns a *set*, because a criss-cross merge (two branches that merge each other) genuinely has two best common ancestors; `merge_base` picks one deterministically (newest timestamp, then id) for callers that need a single answer. Computed in linear passes: intersect ancestor sets, then subtract everything reachable from a proper parent of a common ancestor.
- **A missing parent is an error**, not a silent truncation of history.
- A commit is its own ancestor; parent order is significant (`first_parent_chain` follows the mainline).

## Testing
7 scenario tests (linear, diamond, criss-cross with two bases, unrelated histories, ancestor-as-base, clock skew, missing parent) and 4 property tests (96 random DAGs each, up to 23 commits, 0-3 parents, arbitrary timestamps) against brute-force references written directly from the definitions: ancestors and `is_ancestor`; `log` is a topological order of exactly the ancestor set with no repeats; `merge_bases` equals the O(n^2) definition and is symmetric; when one side is an ancestor of the other it is the only merge base. Mutation-checked: skipping the domination step (returning all common ancestors) and emitting commits before their children are exhausted are both caught.

## Limitations
Queries load and decode commits on each call; there is no commit-graph cache or generation-number index, so very long histories walk linearly. Acceptable here (the store is local and verified); an index would be the first optimization if measured to matter.
