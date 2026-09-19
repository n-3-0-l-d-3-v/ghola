//! Tests for synchronization (ticket 007): correctness against an in-process
//! remote, hardening against a hostile server and a hostile client, and
//! end-to-end runs over distrans's lossy, duplicating, reordering,
//! corrupting simulated network.

use std::collections::BTreeSet;

use object::ObjectId;
use proptest::prelude::*;
use repo::{Files, MergeOutcome, Repo};
use sync::{
    fetch, list_refs, pull, push, serve, Direct, DistransRemote, NetConfig, Remote, SyncError,
    FETCH, PUT_OBJECTS, UPDATE_REF,
};

fn new_repo() -> (tempfile::TempDir, Repo) {
    let d = tempfile::tempdir().unwrap();
    let r = Repo::open(d.path()).unwrap();
    (d, r)
}

/// Builds `commits` commits on `main`, each editing one of `nfiles` files
/// (sizes vary so objects span batches), and returns the tip.
fn history(r: &mut Repo, commits: usize, nfiles: usize, salt: u64) -> ObjectId {
    let mut files = Files::new();
    let mut parent: Vec<ObjectId> = r.get_ref("main").into_iter().collect();
    let mut tip = ObjectId::of(b"none");
    for c in 0..commits {
        let f = (c * 7 + salt as usize) % nfiles;
        files.insert(
            format!("dir{}/file{f}.txt", f % 3).into_bytes(),
            format!("content {salt} {c} {}\n", "x".repeat(50 + (c * 13) % 400)).into_bytes(),
        );
        let tree = r.write_tree(&files).unwrap();
        tip = r
            .write_commit(tree, parent.clone(), "t", &format!("c{c}"), c as u64 + 1)
            .unwrap();
        parent = vec![tip];
        r.set_ref("main", tip).unwrap();
    }
    tip
}

fn ids(r: &Repo) -> BTreeSet<ObjectId> {
    r.object_ids().into_iter().collect()
}

#[test]
fn push_to_an_empty_remote_then_fetch_into_a_fresh_repo_reproduces_everything() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    let (_b, mut bob) = new_repo();
    let tip = history(&mut alice, 12, 6, 1);

    let report = push(&mut alice, &mut Direct(&mut server), "main", false).unwrap();
    assert_eq!(report.objects, ids(&alice).len());
    assert_eq!(server.get_ref("main"), Some(tip));
    assert_eq!(ids(&server), ids(&alice));

    let f = fetch(&mut bob, &mut Direct(&mut server), "origin").unwrap();
    assert_eq!(f.objects, ids(&alice).len());
    assert_eq!(bob.get_ref("remotes/origin/main"), Some(tip));
    assert_eq!(ids(&bob), ids(&alice));
    assert_eq!(
        bob.read_tree(&bob.commit_of(&tip).unwrap().tree)
            .unwrap()
            .len(),
        6
    );

    // Nothing new: a second fetch transfers nothing.
    let again = fetch(&mut bob, &mut Direct(&mut server), "origin").unwrap();
    assert_eq!((again.objects, again.updated.len()), (0, 0));
}

#[test]
fn an_incremental_push_sends_only_what_is_new() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    history(&mut alice, 20, 8, 2);
    push(&mut alice, &mut Direct(&mut server), "main", false).unwrap();

    // One more commit touching one file: a commit, the trees on its path, one blob.
    let mut files = alice
        .read_tree(
            &alice
                .commit_of(&alice.get_ref("main").unwrap())
                .unwrap()
                .tree,
        )
        .unwrap();
    files.insert(b"dir0/file0.txt".to_vec(), b"a small change\n".to_vec());
    let tree = alice.write_tree(&files).unwrap();
    let parent = alice.get_ref("main").unwrap();
    let tip = alice
        .write_commit(tree, vec![parent], "t", "tiny", 99)
        .unwrap();
    alice.set_ref("main", tip).unwrap();

    let report = push(&mut alice, &mut Direct(&mut server), "main", false).unwrap();
    assert!(
        report.objects <= 5,
        "sent {} objects for a one-file change",
        report.objects
    );
    assert_eq!(ids(&server), ids(&alice));
}

#[test]
fn a_non_fast_forward_push_is_refused_unless_forced() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    history(&mut alice, 4, 3, 3);
    push(&mut alice, &mut Direct(&mut server), "main", false).unwrap();

    // Someone else advanced the server's main directly.
    let server_tip = history(&mut server, 2, 3, 99);
    // Alice diverges from the old tip.
    let (_c, mut carol) = new_repo();
    fetch(&mut carol, &mut Direct(&mut alice), "alice").unwrap();
    let diverged = history(&mut alice, 2, 3, 7);
    assert_ne!(diverged, server_tip);

    let err = push(&mut alice, &mut Direct(&mut server), "main", false).unwrap_err();
    assert!(
        matches!(&err, SyncError::Remote(m) if m.contains("stale") || m.contains("non-fast-forward")),
        "{err}"
    );
    assert_eq!(
        server.get_ref("main"),
        Some(server_tip),
        "the remote ref did not move"
    );

    // A forced push overwrites the remote's ref (the CAS still uses the freshly listed value).
    push(&mut alice, &mut Direct(&mut server), "main", true).unwrap();
    assert_eq!(server.get_ref("main"), alice.get_ref("main"));
}

#[test]
fn pull_creates_fast_forwards_merges_and_reports_conflicts() {
    let (_s, mut server) = new_repo();
    let (_a, mut alice) = new_repo();
    let (_b, mut bob) = new_repo();

    // Server has a shared history.
    let mut base_files = Files::new();
    base_files.insert(b"a".to_vec(), b"1\n2\n3\n".to_vec());
    base_files.insert(b"b".to_vec(), b"x\n".to_vec());
    let t = server.write_tree(&base_files).unwrap();
    let base = server.write_commit(t, vec![], "s", "base", 1).unwrap();
    server.set_ref("main", base).unwrap();

    // Alice pulls into an empty repo: creates the branch.
    let o = pull(
        &mut alice,
        &mut Direct(&mut server),
        "origin",
        "main",
        "alice",
        2,
    )
    .unwrap();
    assert_eq!(o, MergeOutcome::FastForward(base));
    assert_eq!(alice.get_ref("main"), Some(base));

    // Bob does the same, then edits file b locally and commits.
    pull(
        &mut bob,
        &mut Direct(&mut server),
        "origin",
        "main",
        "bob",
        2,
    )
    .unwrap();
    let mut bf = base_files.clone();
    bf.insert(b"b".to_vec(), b"x\nbob\n".to_vec());
    let bt = bob.write_tree(&bf).unwrap();
    let bc = bob
        .write_commit(bt, vec![base], "bob", "bob edits b", 3)
        .unwrap();
    bob.set_ref("main", bc).unwrap();

    // The server moves on, editing file a.
    let mut sf = base_files.clone();
    sf.insert(b"a".to_vec(), b"ONE\n2\n3\n".to_vec());
    let st = server.write_tree(&sf).unwrap();
    let sc = server
        .write_commit(st, vec![base], "s", "server edits a", 4)
        .unwrap();
    server.set_ref("main", sc).unwrap();

    // Alice is behind and clean: fast-forward.
    let o = pull(
        &mut alice,
        &mut Direct(&mut server),
        "origin",
        "main",
        "alice",
        5,
    )
    .unwrap();
    assert_eq!(o, MergeOutcome::FastForward(sc));

    // Bob has diverged on a different file: a clean merge commit with two parents.
    let MergeOutcome::Merged(m) = pull(
        &mut bob,
        &mut Direct(&mut server),
        "origin",
        "main",
        "bob",
        6,
    )
    .unwrap() else {
        panic!("expected a clean merge")
    };
    assert_eq!(bob.commit_of(&m).unwrap().parents, vec![bc, sc]);
    let merged = bob.read_tree(&bob.commit_of(&m).unwrap().tree).unwrap();
    assert_eq!(merged[&b"a".to_vec()], b"ONE\n2\n3\n");
    assert_eq!(merged[&b"b".to_vec()], b"x\nbob\n");
    assert_eq!(bob.get_ref("main"), Some(m));

    // Pulling again is a no-op.
    assert_eq!(
        pull(
            &mut alice,
            &mut Direct(&mut server),
            "origin",
            "main",
            "alice",
            7
        )
        .unwrap(),
        MergeOutcome::AlreadyUpToDate
    );
}

// --- hardening -------------------------------------------------------------

#[test]
fn a_server_rejects_updates_to_incomplete_history_and_stale_or_malformed_requests() {
    let (_s, mut server) = new_repo();
    let (_a, mut alice) = new_repo();
    let tip = history(&mut alice, 3, 3, 5);
    let name = "main";
    let update = |old: Option<ObjectId>, new: &ObjectId, force: bool| {
        let mut p = Vec::new();
        p.extend((name.len() as u16).to_le_bytes());
        p.extend(name.as_bytes());
        match old {
            Some(o) => {
                p.push(1);
                p.extend(o.0);
            }
            None => p.push(0),
        }
        p.extend(new.0);
        p.push(u8::from(force));
        p
    };
    // The commit exists on Alice's side only: the server must refuse to point a ref at it.
    let (ok, msg) = serve(&mut server, UPDATE_REF, &update(None, &tip, false));
    assert!(
        !ok && String::from_utf8_lossy(&msg).contains("incomplete"),
        "{}",
        String::from_utf8_lossy(&msg)
    );
    assert_eq!(server.get_ref("main"), None);

    push(&mut alice, &mut Direct(&mut server), "main", false).unwrap();
    let (ok, msg) = serve(&mut server, UPDATE_REF, &update(None, &tip, false));
    assert!(
        !ok && String::from_utf8_lossy(&msg).contains("stale"),
        "creating a ref that exists is stale"
    );
    let (ok, _) = serve(&mut server, UPDATE_REF, &[1, 2, 3]);
    assert!(!ok, "malformed request");
    let (ok, _) = serve(&mut server, 99, &[]);
    assert!(!ok, "unknown method");
}

#[test]
fn a_server_refuses_malformed_and_non_canonical_objects() {
    let (_s, mut server) = new_repo();
    let put = |server: &mut Repo, objs: &[&[u8]]| {
        let mut p = Vec::new();
        p.extend((objs.len() as u32).to_le_bytes());
        for o in objs {
            p.extend((o.len() as u32).to_le_bytes());
            p.extend(*o);
        }
        serve(server, PUT_OBJECTS, &p)
    };
    let (ok, _) = put(&mut server, &[b"\x09garbage"]);
    assert!(!ok);
    // A tree whose entries are out of order is a valid-looking encoding of nothing canonical.
    let mut unsorted = vec![2u8, 2, 0, 0, 0];
    for name in ["b", "a"] {
        unsorted.extend([0u8, 1, 0]);
        unsorted.extend(name.as_bytes());
        unsorted.extend([7u8; 32]);
    }
    let (ok, msg) = put(&mut server, &[&unsorted]);
    assert!(!ok, "{}", String::from_utf8_lossy(&msg));
    assert_eq!(server.stats().objects, 0, "nothing invalid was stored");
    let good = object::Object::Blob(b"fine".to_vec()).encode().unwrap();
    assert!(put(&mut server, &[&good]).0);
    assert_eq!(server.stats().objects, 1);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_requests_never_panic_the_server_and_garbage_stores_nothing(
        method in 0u8..6,
        payload in prop::collection::vec(any::<u8>(), 0..120),
    ) {
        let (_s, mut server) = new_repo();
        let (_ok, _resp) = serve(&mut server, method, &payload);
        if method == PUT_OBJECTS {
            // Random bytes are essentially never a valid object batch; whatever
            // was stored must at least all verify.
            for id in server.object_ids() {
                prop_assert!(server.get(&id).unwrap().is_some());
            }
        }
    }
}

/// A remote that misbehaves when answering FETCH.
enum Mode {
    DropFirstObject,
    FlipABit,
    NoProgress,
}

struct Tamper<'a> {
    inner: Direct<'a>,
    mode: Mode,
}

impl Remote for Tamper<'_> {
    fn call(&mut self, method: u8, payload: &[u8]) -> Result<(bool, Vec<u8>), SyncError> {
        let (ok, mut body) = self.inner.call(method, payload)?;
        if method != FETCH || !ok {
            return Ok((ok, body));
        }
        let total = u32::from_le_bytes(body[0..4].try_into().unwrap());
        let count = u32::from_le_bytes(body[4..8].try_into().unwrap());
        match self.mode {
            Mode::NoProgress => {
                body.truncate(8);
                body[4..8].copy_from_slice(&0u32.to_le_bytes());
            }
            Mode::FlipABit => {
                // Corrupt the last byte of the first object's payload.
                let len = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
                body[12 + len - 1] ^= 1;
            }
            Mode::DropFirstObject => {
                let len = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
                body.drain(8..12 + len);
                body[4..8].copy_from_slice(&(count - 1).to_le_bytes());
                let _ = total;
            }
        }
        Ok((ok, body))
    }
}

#[test]
fn a_lying_truncating_or_corrupting_server_can_never_leave_a_dangling_ref() {
    for mode in [Mode::DropFirstObject, Mode::FlipABit, Mode::NoProgress] {
        let (_s, mut server) = new_repo();
        let (_b, mut bob) = new_repo();
        history(&mut server, 10, 5, 4);
        let result = fetch(
            &mut bob,
            &mut Tamper {
                inner: Direct(&mut server),
                mode,
            },
            "origin",
        );
        assert!(result.is_err(), "a tampering remote must not succeed");
        assert_eq!(
            bob.get_ref("remotes/origin/main"),
            None,
            "no ref may be created"
        );
        // Everything that did get stored is valid (nothing corrupt was accepted under a wrong id).
        for id in bob.object_ids() {
            assert!(bob.get(&id).unwrap().is_some());
        }
    }
}

// --- over distrans ---------------------------------------------------------

#[test]
fn a_full_push_fetch_round_trip_over_a_clean_simulated_network() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    let (_b, mut bob) = new_repo();
    let tip = history(&mut alice, 15, 6, 8);
    {
        let mut net = DistransRemote::new(&mut server, NetConfig::clean(1));
        push(&mut alice, &mut net, "main", false).unwrap();
        assert_eq!(
            net.executions(),
            net.stats().calls,
            "every request ran exactly once"
        );
    }
    {
        let mut net = DistransRemote::new(&mut server, NetConfig::clean(2));
        fetch(&mut bob, &mut net, "origin").unwrap();
    }
    assert_eq!(bob.get_ref("remotes/origin/main"), Some(tip));
    assert_eq!(ids(&bob), ids(&alice));
}

#[test]
fn a_repository_syncs_exactly_across_a_hostile_network() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    let (_b, mut bob) = new_repo();
    let tip = history(&mut alice, 25, 8, 9);
    let pushed = {
        let mut net = DistransRemote::new(&mut server, NetConfig::hostile(42, 0.15));
        push(&mut alice, &mut net, "main", false).unwrap();
        let s = net.stats();
        assert_eq!(
            net.executions(),
            s.calls,
            "duplicated and retried requests must not run twice"
        );
        s
    };
    let dropped = pushed.client_to_server.dropped + pushed.server_to_client.dropped;
    let duplicated = pushed.client_to_server.duplicated + pushed.server_to_client.duplicated;
    let corrupted = pushed.client_to_server.corrupted + pushed.server_to_client.corrupted;
    assert!(
        dropped > 0 && duplicated > 0 && corrupted > 0,
        "faults must really have happened: {pushed:?}"
    );
    assert_eq!(server.get_ref("main"), Some(tip));
    assert_eq!(
        ids(&server),
        ids(&alice),
        "byte-for-byte the same object set"
    );
    {
        let mut net = DistransRemote::new(&mut server, NetConfig::hostile(43, 0.15));
        fetch(&mut bob, &mut net, "origin").unwrap();
    }
    assert_eq!(ids(&bob), ids(&alice));
    for id in bob.object_ids() {
        assert!(
            bob.get(&id).unwrap().is_some(),
            "every received object verifies"
        );
    }
}

#[test]
fn the_same_seed_gives_the_same_transfer() {
    let run = || {
        let (_a, mut alice) = new_repo();
        let (_s, mut server) = new_repo();
        history(&mut alice, 8, 4, 10);
        let mut net = DistransRemote::new(&mut server, NetConfig::hostile(7, 0.1));
        push(&mut alice, &mut net, "main", false).unwrap();
        let s = net.stats();
        (
            s.ticks,
            s.calls,
            s.client_to_server.sent,
            s.server_to_client.sent,
        )
    };
    assert_eq!(run(), run());
}

#[test]
fn a_large_object_crosses_a_hostile_network_intact() {
    let (_a, mut alice) = new_repo();
    let (_s, mut server) = new_repo();
    let mut files = Files::new();
    let big: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 251) as u8).collect();
    files.insert(b"big.bin".to_vec(), big.clone());
    let t = alice.write_tree(&files).unwrap();
    let c = alice.write_commit(t, vec![], "t", "big", 1).unwrap();
    alice.set_ref("main", c).unwrap();
    let mut net = DistransRemote::new(&mut server, NetConfig::hostile(5, 0.05));
    push(&mut alice, &mut net, "main", false).unwrap();
    drop(net);
    let tree = server.commit_of(&c).unwrap().tree;
    assert_eq!(server.read_tree(&tree).unwrap()[&b"big.bin".to_vec()], big);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]

    #[test]
    fn any_history_syncs_exactly_under_arbitrary_faults(
        seed in any::<u64>(),
        loss in 0.0f64..0.25,
        commits in 1usize..14,
        files in 1usize..7,
        salt in any::<u8>(),
    ) {
        let (_a, mut alice) = new_repo();
        let (_s, mut server) = new_repo();
        let (_b, mut bob) = new_repo();
        let tip = history(&mut alice, commits, files, salt as u64);
        {
            let mut net = DistransRemote::new(&mut server, NetConfig::hostile(seed, loss));
            push(&mut alice, &mut net, "main", false).unwrap();
            prop_assert_eq!(net.executions(), net.stats().calls);
        }
        {
            let mut net = DistransRemote::new(&mut server, NetConfig::hostile(seed ^ 1, loss));
            fetch(&mut bob, &mut net, "origin").unwrap();
            let refs = list_refs(&mut net).unwrap();
            prop_assert_eq!(refs.get("main"), Some(&tip));
        }
        prop_assert_eq!(ids(&server), ids(&alice));
        prop_assert_eq!(ids(&bob), ids(&alice));
        prop_assert_eq!(bob.get_ref("remotes/origin/main"), Some(tip));
    }
}
