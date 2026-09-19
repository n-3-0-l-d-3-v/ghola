# Scope — ghola

## CORE
- Content-addressed immutable objects (blob, tree, commit) with a from-scratch SHA-256, stored on sietch; refs; the commit DAG (log, ancestors, merge base).
- Diffs computed on demand (Myers line diff, tree diff); no diff is ever stored as history.
- Working tree snapshot/checkout and a real `ghola` CLI.

## EXTENSION
- Three-way merge with conflict reporting.
- Synchronization (push/pull of missing objects) over distrans's hostile channel.

## EXPERIMENT (only if CORE and EXTENSION are healthy)
- Pack files / delta compression, shallow clones, signed commits.
