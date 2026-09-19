//! End-to-end tests of `ghola fetch / push / pull` between two real
//! repositories on disk, carried over distrans's simulated network.

use std::fs;
use std::path::Path;
use std::process::Command;

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn g(dir: &Path, args: &[&str]) -> Out {
    let o = Command::new(env!("CARGO_BIN_EXE_ghola"))
        .current_dir(dir)
        .args(args)
        .env("GHOLA_AUTHOR", "Tester <t@example.com>")
        .env("GHOLA_TIMESTAMP", "1000")
        .output()
        .unwrap();
    Out {
        code: o.status.code().unwrap(),
        out: String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n"),
        err: String::from_utf8_lossy(&o.stderr).replace("\r\n", "\n"),
    }
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let o = g(dir, args);
    assert_eq!(o.code, 0, "ghola {args:?} failed: {}{}", o.out, o.err);
    o.out
}

fn write(dir: &Path, name: &str, content: &str) {
    let p = dir.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name))
        .unwrap()
        .replace("\r\n", "\n")
}

fn init(root: &Path, name: &str) -> std::path::PathBuf {
    let d = root.join(name);
    fs::create_dir_all(&d).unwrap();
    ok(&d, &["init"]);
    d
}

#[test]
fn push_then_pull_moves_a_project_between_repositories() {
    let root = tempfile::tempdir().unwrap();
    let (origin, alice, bob) = (
        init(root.path(), "origin"),
        init(root.path(), "alice"),
        init(root.path(), "bob"),
    );

    write(&alice, "README", "hello\n");
    write(&alice, "src/main.rs", "fn main() {}\n");
    ok(&alice, &["commit", "-m", "first"]);
    let pushed = ok(&alice, &["push", "../origin"]);
    assert!(pushed.starts_with("Pushed main: "), "{pushed}");

    let pulled = ok(&bob, &["pull", "../origin"]);
    assert!(pulled.contains("main is now at "), "{pulled}");
    assert_eq!(read(&bob, "README"), "hello\n");
    assert_eq!(read(&bob, "src/main.rs"), "fn main() {}\n");
    assert_eq!(
        ok(&bob, &["log", "--oneline"]),
        ok(&alice, &["log", "--oneline"]),
        "identical history"
    );
    assert!(ok(&bob, &["pull", "../origin"]).contains("Already up to date."));

    // Bob edits and pushes; Alice pulls it back.
    write(&bob, "README", "hello\nfrom bob\n");
    ok(&bob, &["commit", "-m", "bob edit"]);
    ok(&bob, &["push", "../origin"]);
    ok(&alice, &["pull", "../origin"]);
    assert_eq!(read(&alice, "README"), "hello\nfrom bob\n");
    let _ = origin;
}

#[test]
fn a_transfer_over_a_lossy_corrupting_network_arrives_intact() {
    let root = tempfile::tempdir().unwrap();
    init(root.path(), "origin");
    let (alice, bob) = (init(root.path(), "alice"), init(root.path(), "bob"));
    for i in 0..8 {
        write(
            &alice,
            &format!("dir{}/file{i}.txt", i % 3),
            &format!("file {i}\n{}\n", "data ".repeat(40 + i * 11)),
        );
        ok(&alice, &["commit", "-m", &format!("commit {i}")]);
    }
    let pushed = ok(
        &alice,
        &["push", "../origin", "--loss", "0.2", "--seed", "9"],
    );
    assert!(pushed.contains("dropped"), "{pushed}");
    let dropped: u64 = pushed
        .split('(')
        .next_back()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        dropped > 0,
        "the network really dropped datagrams: {pushed}"
    );

    ok(
        &bob,
        &["pull", "../origin", "--loss", "0.2", "--seed", "10"],
    );
    assert_eq!(
        ok(&bob, &["log", "--oneline"]),
        ok(&alice, &["log", "--oneline"])
    );
    for i in 0..8 {
        let name = format!("dir{}/file{i}.txt", i % 3);
        assert_eq!(read(&bob, &name), read(&alice, &name), "{name}");
    }
}

#[test]
fn a_diverged_pull_merges_cleanly_and_a_conflicting_one_says_how_to_resolve() {
    let root = tempfile::tempdir().unwrap();
    init(root.path(), "origin");
    let (alice, bob) = (init(root.path(), "alice"), init(root.path(), "bob"));
    write(&alice, "a", "1\n2\n3\n");
    write(&alice, "b", "x\n");
    ok(&alice, &["commit", "-m", "base"]);
    ok(&alice, &["push", "../origin"]);
    ok(&bob, &["pull", "../origin"]);

    // Alice edits a and pushes; Bob edits b locally: a clean merge on pull.
    write(&alice, "a", "ONE\n2\n3\n");
    ok(&alice, &["commit", "-m", "alice a"]);
    ok(&alice, &["push", "../origin"]);
    write(&bob, "b", "x\nbob\n");
    ok(&bob, &["commit", "-m", "bob b"]);
    ok(&bob, &["pull", "../origin"]);
    assert_eq!(read(&bob, "a"), "ONE\n2\n3\n");
    assert_eq!(read(&bob, "b"), "x\nbob\n");
    assert!(ok(&bob, &["log"]).contains("Merge:  "));
    // Bob's merge is a descendant of the remote, so he can push it.
    ok(&bob, &["push", "../origin"]);

    // Now both edit the same line of a: Alice's push is rejected as non-fast-forward,
    // and pulling reports the conflict and how to resolve it.
    write(&bob, "a", "BOB\n2\n3\n");
    ok(&bob, &["commit", "-m", "bob a"]);
    ok(&bob, &["push", "../origin"]);
    write(&alice, "a", "ALICE\n2\n3\n");
    ok(&alice, &["commit", "-m", "alice a2"]);
    let rejected = g(&alice, &["push", "../origin"]);
    assert_eq!(rejected.code, 1);
    assert!(rejected.err.contains("remote refused"), "{}", rejected.err);
    let conflict = g(&alice, &["pull", "../origin"]);
    assert_eq!(conflict.code, 1);
    assert!(conflict.out.contains("CONFLICT"), "{}", conflict.out);
    assert!(
        conflict.err.contains("ghola merge remotes/origin/main"),
        "{}",
        conflict.err
    );
    // The suggested command really does resolve it, with markers in the file.
    let merge = g(&alice, &["merge", "remotes/origin/main"]);
    assert_eq!(merge.code, 1);
    assert!(read(&alice, "a").contains("<<<<<<< ours"));
}

#[test]
fn a_local_edit_blocks_a_pull_and_bad_arguments_are_reported() {
    let root = tempfile::tempdir().unwrap();
    init(root.path(), "origin");
    let (alice, bob) = (init(root.path(), "alice"), init(root.path(), "bob"));
    write(&alice, "f", "1\n");
    ok(&alice, &["commit", "-m", "one"]);
    ok(&alice, &["push", "../origin"]);
    ok(&bob, &["pull", "../origin"]);
    write(&alice, "f", "2\n");
    ok(&alice, &["commit", "-m", "two"]);
    ok(&alice, &["push", "../origin"]);

    write(&bob, "f", "local\n");
    let blocked = g(&bob, &["pull", "../origin"]);
    assert_eq!(blocked.code, 1);
    assert!(blocked.err.contains("local changes"), "{}", blocked.err);
    assert_eq!(read(&bob, "f"), "local\n", "the local edit is untouched");

    assert_eq!(g(&bob, &["push", "../nowhere"]).code, 1);
    assert_eq!(g(&bob, &["push"]).code, 1);
    assert_eq!(g(&bob, &["push", "../origin", "--loss", "2"]).code, 1);
    assert_eq!(g(&bob, &["push", "../origin", "--loss", "abc"]).code, 1);
}
