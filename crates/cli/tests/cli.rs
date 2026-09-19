//! End-to-end tests of the real `ghola` binary over real directories.

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
    assert_eq!(o.code, 0, "ghola {args:?} failed: {}", o.err);
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

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    ok(d.path(), &["init"]);
    d
}

#[test]
fn init_commit_status_diff_log() {
    let d = repo();
    let p = d.path();
    write(p, "a.txt", "one\ntwo\nthree\n");
    let out = ok(p, &["commit", "-m", "first"]);
    assert!(out.starts_with("[main "), "{out}");
    assert!(out.trim_end().ends_with("] first"));
    assert!(ok(p, &["status"]).contains("nothing to commit, working tree clean"));

    write(p, "a.txt", "one\nTWO\nthree\n");
    write(p, "dir/new.txt", "hi\n");
    let status = ok(p, &["status"]);
    assert!(
        status.contains("M a.txt") && status.contains("A dir/new.txt"),
        "{status}"
    );
    let diff = ok(p, &["diff"]);
    assert!(diff.contains("--- a/a.txt\n+++ b/a.txt"), "{diff}");
    assert!(diff.contains("-two\n+TWO"), "{diff}");
    assert!(diff.contains("--- /dev/null\n+++ b/dir/new.txt"), "{diff}");

    ok(p, &["commit", "-m", "second"]);
    let log = ok(p, &["log", "--oneline"]);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].ends_with("second") && lines[1].ends_with("first"),
        "{log}"
    );
    let full = ok(p, &["log"]);
    assert!(full.contains("Author: Tester <t@example.com>") && full.contains("    second"));
    let between = ok(p, &["diff", "HEAD~1", "HEAD"]);
    assert!(between.contains("+TWO"), "{between}");
}

#[test]
fn checkout_moves_between_revisions_and_keeps_untracked_files() {
    let d = repo();
    let p = d.path();
    write(p, "f", "v1\n");
    ok(p, &["commit", "-m", "one"]);
    write(p, "f", "v2\n");
    write(p, "extra/only-in-v2", "x\n");
    ok(p, &["commit", "-m", "two"]);
    write(p, "untracked.txt", "mine\n");

    let out = ok(p, &["checkout", "HEAD~1"]);
    assert!(out.starts_with("HEAD is now at "), "{out}");
    assert_eq!(read(p, "f"), "v1\n");
    assert!(
        !p.join("extra").exists(),
        "the emptied directory is removed"
    );
    assert_eq!(
        read(p, "untracked.txt"),
        "mine\n",
        "untracked files survive"
    );
    assert!(ok(p, &["status"]).contains("HEAD detached at"));

    assert!(ok(p, &["checkout", "main"]).contains("Switched to branch main"));
    assert_eq!(read(p, "f"), "v2\n");
    assert_eq!(read(p, "extra/only-in-v2"), "x\n");
}

#[test]
fn checkout_refuses_to_overwrite_local_changes_unless_forced() {
    let d = repo();
    let p = d.path();
    write(p, "f", "v1\n");
    ok(p, &["commit", "-m", "one"]);
    write(p, "f", "v2\n");
    ok(p, &["commit", "-m", "two"]);
    write(p, "f", "local edit\n");
    let o = g(p, &["checkout", "HEAD~1"]);
    assert_eq!(o.code, 1);
    assert!(
        o.err.contains("local changes would be overwritten: f"),
        "{}",
        o.err
    );
    assert_eq!(read(p, "f"), "local edit\n", "nothing was touched");
    ok(p, &["checkout", "HEAD~1", "--force"]);
    assert_eq!(read(p, "f"), "v1\n");
}

#[test]
fn fast_forward_and_clean_three_way_merges() {
    let d = repo();
    let p = d.path();
    write(p, "a", "1\n2\n3\n4\n5\n");
    write(p, "b", "b1\n");
    ok(p, &["commit", "-m", "base"]);
    ok(p, &["branch", "feature"]);

    // main moves on and edits b; feature edits a. Neither touches the other's file.
    write(p, "b", "b1\nb2\n");
    ok(p, &["commit", "-m", "main edits b"]);
    ok(p, &["checkout", "feature"]);
    write(p, "a", "ONE\n2\n3\n4\n5\n");
    ok(p, &["commit", "-m", "feature edits a"]);
    ok(p, &["checkout", "main"]);

    let out = ok(p, &["merge", "feature", "-m", "join"]);
    assert!(out.starts_with("Merge made: "), "{out}");
    assert_eq!(read(p, "a"), "ONE\n2\n3\n4\n5\n");
    assert_eq!(read(p, "b"), "b1\nb2\n");
    let log = ok(p, &["log"]);
    assert!(
        log.contains("Merge:  "),
        "a merge commit lists both parents:\n{log}"
    );
    assert_eq!(ok(p, &["log", "--oneline"]).lines().count(), 4);

    // feature is now fully contained; merging again changes nothing.
    assert!(ok(p, &["merge", "feature"]).contains("Already up to date."));

    // feature is behind main, so merging main into it is a fast-forward.
    ok(p, &["checkout", "feature"]);
    let ff = ok(p, &["merge", "main"]);
    assert!(ff.starts_with("Fast-forward to "), "{ff}");
    assert_eq!(read(p, "b"), "b1\nb2\n");
}

#[test]
fn a_conflicting_merge_is_reported_resolved_by_hand_and_committed_with_two_parents() {
    let d = repo();
    let p = d.path();
    write(p, "f", "a\nb\nc\n");
    ok(p, &["commit", "-m", "base"]);
    ok(p, &["branch", "other"]);
    write(p, "f", "a\nMAIN\nc\n");
    ok(p, &["commit", "-m", "main change"]);
    ok(p, &["checkout", "other"]);
    write(p, "f", "a\nOTHER\nc\n");
    ok(p, &["commit", "-m", "other change"]);
    ok(p, &["checkout", "main"]);

    let o = g(p, &["merge", "other"]);
    assert_eq!(o.code, 1);
    assert!(
        o.out.contains("CONFLICT") && o.out.contains('f'),
        "{}",
        o.out
    );
    assert!(o.err.contains("automatic merge failed"), "{}", o.err);
    let f = read(p, "f");
    assert!(
        f.contains("<<<<<<< ours\nMAIN\n=======\nOTHER\n>>>>>>> theirs"),
        "conflict markers in the working file:\n{f}"
    );
    assert!(ok(p, &["status"]).contains("Merging "));
    assert_eq!(
        g(p, &["checkout", "other"]).code,
        1,
        "no checkout during a merge"
    );

    write(p, "f", "a\nMAIN+OTHER\nc\n");
    ok(p, &["commit", "-m", "resolved"]);
    assert!(!ok(p, &["status"]).contains("Merging"));
    let log = ok(p, &["log"]);
    assert!(
        log.contains("Merge:  ") && log.contains("    resolved"),
        "{log}"
    );
    assert_eq!(read(p, "f"), "a\nMAIN+OTHER\nc\n");
}

#[test]
fn aborting_a_conflicted_merge_restores_the_branch() {
    let d = repo();
    let p = d.path();
    write(p, "f", "x\n");
    ok(p, &["commit", "-m", "base"]);
    ok(p, &["branch", "other"]);
    write(p, "f", "MAIN\n");
    ok(p, &["commit", "-m", "m"]);
    ok(p, &["checkout", "other"]);
    write(p, "f", "OTHER\n");
    ok(p, &["commit", "-m", "o"]);
    ok(p, &["checkout", "main"]);
    assert_eq!(g(p, &["merge", "other"]).code, 1);
    write(p, "untracked", "keep me\n");
    assert!(ok(p, &["merge", "--abort"]).contains("Merge aborted"));
    assert_eq!(read(p, "f"), "MAIN\n");
    assert_eq!(read(p, "untracked"), "keep me\n");
    assert_eq!(g(p, &["merge", "--abort"]).code, 1, "nothing left to abort");
}

#[test]
fn branch_listing_and_errors() {
    let d = repo();
    let p = d.path();
    assert_eq!(g(p, &["branch", "early"]).code, 1, "no commits yet");
    write(p, "f", "1\n");
    ok(p, &["commit", "-m", "c"]);
    ok(p, &["branch", "topic"]);
    let list = ok(p, &["branch"]);
    assert!(
        list.contains("* main") && list.contains("  topic"),
        "{list}"
    );
    assert_eq!(g(p, &["branch", "topic"]).code, 1, "already exists");
}

#[test]
fn error_handling_and_usage() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let o = g(p, &["status"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("not a ghola repository"), "{}", o.err);
    ok(p, &["init"]);
    assert_eq!(g(p, &["init"]).code, 1, "already initialized");
    assert_eq!(
        g(p, &["commit", "-m", "x"]).code,
        0,
        "an empty first commit is allowed"
    );
    let o = g(p, &["commit", "-m", "again"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("nothing to commit"), "{}", o.err);
    assert_eq!(g(p, &["commit"]).code, 1, "needs -m");
    assert_eq!(g(p, &["checkout", "nosuchthing"]).code, 1);
    assert_eq!(g(p, &["diff", "HEAD~9"]).code, 1);
    assert_eq!(g(p, &["frobnicate"]).code, 2);
    assert_eq!(g(p, &[]).code, 2);
    assert_eq!(g(p, &["--help"]).code, 0);
}

#[test]
fn history_is_found_from_a_subdirectory() {
    let d = repo();
    let p = d.path();
    write(p, "sub/deep/file", "x\n");
    ok(p, &["commit", "-m", "deep"]);
    let out = ok(&p.join("sub").join("deep"), &["log", "--oneline"]);
    assert!(out.contains("deep"), "{out}");
}
