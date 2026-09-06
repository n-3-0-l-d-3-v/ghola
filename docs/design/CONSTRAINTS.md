# Constraints — THE HISTORY

## Primary constraint

Every object is immutable and content-addressed. History forms a DAG. Diffs are computed on demand, never stored as the primary historical representation.

## What it forces

Content-addressed object storage on top of sietch, DAG traversal, and network synchronization over distrans.

## Research question

How naturally does content-addressed immutable storage support version history without storing diffs?

## What is explicitly out of scope

See the root [SCOPE.md](../../SCOPE.md) for the CORE / EXTENSION / EXPERIMENT
classification that applies to this repo.
