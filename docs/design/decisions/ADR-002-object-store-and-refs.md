# ADR-002: A verified object store on sietch, snapshots as trees, and deduplication by construction

## Status
Accepted

## Decisions
- **One `sietch::Store` holds everything**: objects under `o` + id, refs under `r/<name>`, and `HEAD`. Objects are never modified; refs and HEAD are the only mutable state, and on an append-only store even they are new versions rather than overwrites.
- **Reads verify.** `get` hashes the bytes it read and compares the digest to the id it was asked for; a mismatch is `RepoError::Corrupt`, never data. Tested by overwriting a stored value behind the repo's back with a one-bit-flipped copy. Mutation-checked: removing the comparison fails that test.
- **Writes are idempotent and cost nothing when redundant.** `put` skips the append if the key exists. The first version of the test only compared object *counts*, which cannot see a redundant append (re-putting the same key leaves the count unchanged), so a mutant that always wrote survived. A test now compares on-disk byte totals before and after re-putting an object, and kills that mutant. Reported because it is a case where a test looked adequate and was not.
- **Snapshots are nested trees built from a flat `path -> bytes` map** (`write_tree` / `read_tree`). Tree ids depend only on content, so deduplication is not a feature that runs: identical files and identical subdirectories are the same object wherever and whenever they appear. Invalid paths (empty components, `.`, `..`, or a name used as both a file and a directory) are errors, courtesy of the object layer's name rules.
- **Refs**: names limited to `[A-Za-z0-9._/-]` with no leading/trailing/double slash and no `..`. HEAD is either a branch name (which may have no commits yet, the "unborn" state) or a detached commit id.

## Measured (the research question: how naturally does this support history without diffs?)
100 commits of a 50-file, 50 KiB tree, one 1 KiB file edited per commit, all stored as full snapshots: **322 objects and 188,062 bytes**, against 5,120,000 bytes if every commit stored a full copy: **3.7%**. Every one of the 100 historical snapshots reads back in full, directly, with no chain of diffs to replay. The cost of an edit is exactly the objects on the edited file's path (one blob plus one tree per directory level up to the root), which a property test asserts for arbitrary snapshots and arbitrary edits.

Honest limits: a whole-file granularity means editing one byte of a 1 MiB file stores a new 1 MiB blob (no delta compression; pack files are an EXPERIMENT item in `SCOPE.md`), and deleting history is not supported because nothing is ever removed.

## Testing
13 repo unit tests and 5 property/measurement tests (128 cases each): snapshots round-trip including across a real close and reopen and every stored object still verifies; rewriting a snapshot stores nothing; changing one file adds at most `depth + 1` objects; distinct snapshots get distinct root ids.
