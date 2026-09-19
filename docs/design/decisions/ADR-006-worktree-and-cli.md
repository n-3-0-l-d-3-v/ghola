# ADR-006: The working directory as a snapshot, and a real `ghola` command line

## Status
Accepted

## Decisions
- **`worktree`** turns a directory into a `Files` snapshot (`read_files`) and moves a directory from one snapshot to another (`apply(root, from, to)`): delete what `to` lacks, then write what is new or changed. Consequences, each tested: untracked files are never touched (`apply` only looks at paths that differ between the two snapshots); a path can change between file and directory in either direction; an emptied directory is pruned; symlinks are skipped; the `.ghola` metadata directory is never read as content and never writable through a repository path.
- **Repository paths are untrusted input.** A crafted tree must not be able to write outside the working directory, so every path is validated before anything touches the disk: empty components, `.`, `..`, backslashes, drive colons, NULs and the metadata directory are rejected, and *all* paths in both snapshots are validated before the first write, so a bad path leaves the directory unchanged. Mutation-checked: dropping the `..` check fails the traversal test.
- **No staging area.** `ghola commit` snapshots every file in the working directory (excluding `.ghola`), like `git commit -a` plus adding new files. That is a deliberate simplification: one fewer concept, and the snapshot model of the object store makes it natural. `status` and `diff` compare the working directory to HEAD, or two revisions to each other.
- **Time enters only in the CLI.** The library crates take timestamps as data; the binary reads the system clock unless `GHOLA_TIMESTAMP` is set, which the end-to-end tests use for reproducible commits.
- **Checkout and merge protect local work precisely**: they refuse only if a *tracked path they would change* has local modifications (`local changes would be overwritten: f`), not merely because untracked files exist, and `--force` overrides checkout. A conflicted merge writes marker files into the working directory, records `MERGE_HEAD` in the repository, and blocks checkout until the user commits the resolution (which then becomes a two-parent commit) or runs `merge --abort` (which restores tracked files and leaves untracked ones alone).
- **Revisions**: `HEAD`, a branch name, a commit id or unique prefix of 4+ hex digits (ambiguous or non-commit prefixes are errors), each optionally followed by `~N` first-parent steps.

## Testing
Worktree: 5 unit tests and 3 property tests over real temporary directories: an arbitrary snapshot round-trips through write and read, arbitrary snapshot-to-snapshot transitions land exactly on the target and back, and an untracked file survives any transition.
CLI: 9 end-to-end tests that spawn the real binary: init/commit/status/diff/log; checkout between revisions with untracked files kept and emptied directories removed; refusal to overwrite local changes and `--force`; fast-forward, already-up-to-date and clean three-way merges (merge commit shows both parents); a conflicted merge (marker file, non-zero exit, `Merging` in status, no checkout, hand-resolved commit with two parents); `merge --abort`; branches; error and usage exit codes (1 for failed commands, 2 for usage errors); finding the repository from a subdirectory.
Mutation-checked: removing the local-changes guard fails the refusal test; forgetting to record `MERGE_HEAD` on conflict fails two merge tests.

## Limitations
No staging area, no `.gholaignore`, no file modes or empty directories, no remotes yet (ticket 007), no `stash`/`rebase`/`reset`, no packed storage, and the single-writer sietch store means two `ghola` processes must not run in one repository at the same time (there is no lock file; running two at once is unsupported). Merge output does not yet name the base and shows conflicts by kind and path only.
