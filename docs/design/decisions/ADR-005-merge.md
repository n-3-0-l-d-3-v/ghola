# ADR-005: Three-way merge as a lossless alignment, with deliberately conservative conflicts

## Status
Accepted

## Decisions
- **The line-level merge (`diff::merge3`) aligns the three inputs into regions** rather than producing text directly: `Same(lines)` (identical in base, ours and theirs) and `Changed { base, ours, theirs }`. Concatenating each of the three columns reproduces exactly `base`, `ours` and `theirs` (property-tested for arbitrary triples), so the alignment cannot silently drop or invent a line. A changed region is then classified: only ours changed it, only theirs did, both made the identical change (taken once), or a conflict.
- **Conflicts are conservative on purpose.** Changes conflict when their base ranges intersect *including touching*: edits to adjacent lines, or two insertions at the same point, are reported instead of combined in an arbitrary order. A false conflict costs a human a glance; a false clean merge can silently ship the wrong code. A mutation that relaxed "touching" to "overlapping" is caught by three tests.
- **Snapshot merge per path** (`repo::merge_files`): identical on both sides (including both deleting), or only one side changed it (including a deletion), resolve without asking; otherwise text is merged line by line (clean result used, conflicting result written with `<<<<<<<`/`=======`/`>>>>>>>` markers and reported as `Content` or `BothAdded`), one side deleting what the other modified is `ModifyDelete` (the surviving version is kept so nothing is lost), and a binary file (contains NUL) changed on both sides is `Binary` (ours kept, no markers injected into binary data).
- **Commit merge** decides already-up-to-date, fast-forward, clean merge (a new commit with parents `[ours, theirs]`), or conflicts (returned, nothing written). It never moves a ref: the caller decides, which keeps merge a pure function of the store.
- Unrelated histories merge against an empty base.

## Testing
9 line-merge unit tests and 6 property tests (512 cases each): lossless alignment; identical sides and one-sided changes are identities; merging is symmetric (swapping sides swaps every region's sides); **edits built to be disjoint always merge cleanly to exactly the constructed result, and edits built to collide conflict in exactly the constructed segments** (segments separated by an unedited separator with fresh replacement values, which makes the minimal alignment unique so the property is exact); marker rendering has one marker set per conflict. Snapshot level: 9 unit tests (every conflict kind, deletions, identical additions, commit outcomes, unrelated histories) and 5 property tests: identities, symmetry (with `ours_deleted` flipping correctly), **a reported conflict implies both sides really changed the path and differently** (soundness), disjoint-path edits combine exactly, and `merge_commits` writes the merged tree with the right parents.
Mutation-checked: relaxing the touching rule; breaking one side's reconstruction; dropping the "other side unchanged" shortcut; and swapping the `ours_deleted` flag are all caught.

## Limitations, stated plainly
- **Criss-cross merges** (several best common ancestors) use one deterministically chosen base instead of merging the bases recursively as git's `recursive`/`ort` strategies do; results there can differ from git's and may produce conflicts git would avoid.
- No rename detection: a file renamed on one side and edited on the other shows as a delete plus an add.
- Line granularity only; no whitespace-insensitive or semantic merging.
- Conflict markers omit the base section (`diff3` style).
