# ADR-004: Diffs are computed on demand, never stored

## Status
Accepted

## Decisions
- **No diff is ever the stored form of history.** Commits point at whole-snapshot trees (ADR-002); a diff is a function of two snapshots computed when asked. This is the phase's constraint made concrete, and it is why deduplication (3.7% of naive in ADR-002) and O(1)-per-revision access do not require replaying a chain of deltas.
- **Myers' O(ND) algorithm**, generic over any comparable element, returning a *self-contained* `Patch`: `Equal(n) / Delete(n) / Insert(items)` runs, with inserted elements carried inside so `apply(patch, a)` needs nothing else. `apply` refuses (typed error) a source that the patch does not consume exactly, rather than guessing. `invert` builds the undo patch from the original source.
- **Common prefix and suffix are trimmed first.** That cannot change the edit distance and shrinks the quadratic part of the search; it is checked implicitly by the minimality property.
- **Text diffs are line-based and lines keep their `\n`.** So a file with and without a trailing newline differ for real, concatenating the lines returns the original bytes exactly, and unified output emits the `\ No newline at end of file` marker instead of hiding the difference.
- **Unified format** with configurable context: hunks are merged when their context windows overlap *or touch*; a pure insertion at the top of a file uses git's `-0,0` convention.
- **Tree diff skips unchanged subtrees by comparing ids** and never reads them, because equal ids mean identical contents all the way down. The result is a path-sorted list of Added / Removed / Modified with the real blob ids on each side (whole directories that appear or vanish are reported file by file; a path that changes between file and directory is a remove plus an add).

## Testing
Myers/text: 13 unit tests (the worked example from Myers' paper has edit distance 5; empty and identical inputs; single change; wrong-source errors; inversion; hand-checked unified output; hunk splitting and merging; zero-start insertion; no-newline marker) and 9 property tests at 512 cases: apply reconstructs the target; **the edit script is minimal, checked against an O(nm) LCS dynamic-programming reference over a 4-letter alphabet** (the hard case, with many repeats); patches invert; self-diff is a single keep; scripts are coalesced with no empty or repeated runs; text patches reconstruct exactly; unified headers' counts match their bodies; with enough context the unified body reproduces both files exactly.
Tree diff: 4 unit tests and 3 property/scenario tests: result equals a brute-force comparison of flattened snapshots and reports the right blob ids; diffing is antisymmetric; **the whole unchanged subtree's objects are deleted behind the repository's back and the diff still succeeds** (reading the full tree fails), which proves the subtree is genuinely not read.
Mutation-checked: letting Myers' snake step take one diagonal move instead of a loop (caught by minimality); merging windows only when they overlap and not when they touch (a first version of the test suite missed this, so a test for touching windows was added and kills it); always recursing instead of comparing ids (caught by the unread-subtree test and others).

## Two test-design notes, reported honestly
- The reconstruction-from-unified property first used `prop_assume!` to discard inputs lacking trailing newlines and aborted for too many rejects (1,024) rather than failing; the fix was to generate whole-line files by construction, not to filter.
- The hunk-merge mutant survived the first suite because nothing tested windows that merely touch. Adding that test is what killed it.

## Limitations
Line-granularity only (no word or character diffs); no rename or copy detection; no minimal-*hunk* heuristics beyond Myers' shortest script (diff output can differ from git's `--patience`/`--histogram` choices while being equally short); the O(ND) trace keeps a snapshot per edit-distance step, fine for source files but memory-hungry for very large, very different inputs.
