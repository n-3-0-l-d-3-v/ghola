# ADR-007: Synchronization over distrans, with an untrusted peer on both sides

## Status
Accepted (closes Phase 8)

## Decisions
- **Three layers.** `client` (`fetch`, `push`, `pull` against any `Remote`), a request protocol with a *pure* server function `serve(repo, method, payload)`, and `net::DistransRemote`, which carries the same protocol over distrans RPC, over a reliable transport connection, over a hostile simulated channel (loss, duplication, reordering, corruption, truncation, virtual time, reproducible from a seed). Because `serve` is separate from the transport, the protocol logic is tested with an in-process `Direct` remote and the transport is tested separately, not tangled.
- **Four methods**: list refs; fetch (wants + haves, paged by offset, at most 4 KiB of objects per response, stateless: the server recomputes the sorted plan each call); put objects; update ref. The plan is `reachable(wants) minus reachable(haves)`, so an incremental push or fetch sends only what is new (a one-file change sends at most 5 objects, tested).
- **The peer is untrusted in both directions.**
  - *A malicious server* cannot leave a client with a dangling ref: every received object is validated as a canonical object and stored under the id **the client computes**, refs only move after every wanted commit's whole closure is verifiably present (`is_complete`), and a response that makes no progress is an error. Tested with servers that drop an object from a batch, flip a bit inside one, or claim progress while sending nothing: each fetch fails, no ref is created, everything stored still verifies.
  - *A malicious client* cannot corrupt the server: pushed bytes must be the canonical encoding of a valid object (garbage and unsorted trees are refused and store nothing), a ref only moves if the new commit's full history is present, the caller's belief about the old value must be current (compare-and-set, "stale" otherwise), and a non-fast-forward move is refused unless forced. Arbitrary request bytes never panic the server (property-tested, 256 cases).
- **Exactly-once per request under retries.** distrans's RPC server deduplicates by request id, so a retried or duplicated request never runs its handler twice. Asserted on every network run: `executions == calls`, including under 15-25% loss with duplication and corruption.
- **Refs move last.** Objects are content-addressed, so storing extra objects is harmless and idempotent; only the final ref update is a state change, and it is the only step that can be refused, which makes a failed or interrupted transfer safe to retry.

## Findings (reported plainly)
1. **The RPC layer's fixed retry deadline collapses under large messages.** The first version of the 300 KB-object test failed with "request gave up after 200 attempts". `RpcClient` resends the *whole* request every `retry_deadline` ticks on top of the transport's own reliable retransmission; with a fixed 60-tick deadline, a request that needs longer than that to drain is re-queued faster than the link can carry it, and the connection collapses under its own retries. The fix here is at the call site: the deadline grows with the payload (`base + 4 * len` ticks), since loss is already handled by the transport. The underlying issue belongs to distrans's RPC layer (retries should account for a still-draining send queue); it is recorded here rather than silently patched around.
2. **Latency, not bandwidth, is what loss costs.** Pushing a 40-commit history (160 objects, 93,622 bytes) to an empty remote, one seed per row:

   | Network | Ticks | Datagrams sent | Dropped | Corrupted | Slowdown |
   |---|---|---|---|---|---|
   | clean | 36 | 475 | 0 | 0 | 1x |
   | 10% loss (plus duplication, reordering, corruption) | 4,632 | 660 | 64 | 22 | 129x |
   | 20% loss | 18,163 | 706 | 143 | 20 | 505x |
   | 30% loss | 45,104 | 781 | 251 | 13 | 1,253x |

   Datagram count grows only 1.4x to 1.6x while time grows by two to three orders of magnitude: the cost is timeout stalls (the transport's retransmission timer backs off) multiplied by the strictly one-request-at-a-time protocol, not extra bytes. Pipelining batches or a lower retransmission ceiling would attack that; neither is built. Single runs of a deterministic simulator, virtual ticks, no wall-clock claim.
3. One test failure was my own wrong expectation, not a bug: I expected a forced push with a stale base to fail, but `push` lists the remote's current ref immediately before updating, so a forced push correctly succeeds and overwrites. The test now asserts that.

## Testing
13 sync tests: full push/fetch round trip reproduces the identical object set; incremental push sends only new objects; non-fast-forward refused unless forced, remote ref unmoved; pull creates, fast-forwards, merges cleanly (two parents) and is a no-op when current; the server refuses stale, malformed, unknown-method and incomplete-history updates and non-canonical or garbage objects; three tampering servers; property test of arbitrary requests; a clean-network round trip; a 15%-loss/duplicating/corrupting run that must really have dropped, duplicated and corrupted datagrams and still yield the byte-identical object set; same-seed determinism; a 300 KB object across a lossy network; and a property test (20 cases) that any history syncs exactly under arbitrary seeds and up to 25% loss. 4 end-to-end tests of the real `ghola` binary between repositories on disk, including a transfer at 20% loss that verifiably dropped datagrams, a clean diverged pull, a conflicting pull that names the resolving command, and a local edit blocking a pull.
Mutation-checked: skipping the client's completeness check, skipping the server's completeness check, skipping compare-and-set, and skipping the fast-forward rule are each caught.

## Limitations
The "network" is distrans's *simulated* channel and remotes are other repositories on the local disk; there are no real sockets, authentication, or encryption. Calls are strictly sequential (no pipelining). No shallow clones, no delta/pack transfer (each object travels whole, so a large file resent per revision is large), no ref deletion, no tag objects, and remote-tracking refs live under `remotes/<name>/`. A single huge object must fit in one RPC message.

## Phase 8 summary
Objects and hashing, a verified store, the commit DAG, diffs computed on demand, three-way merge, a working-tree CLI, and synchronization over the hostile network are done. The research question is answered concretely: content addressing gives deduplicated whole-snapshot history with no stored diffs (3.7% of naive over 100 commits, ADR-002), cheap unchanged-subtree skipping in diffs (proved by deleting the subtree), and safe, verifiable synchronization, at the price of whole-file granularity for every change.
