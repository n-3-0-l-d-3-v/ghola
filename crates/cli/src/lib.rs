//! The `ghola` command line: a small, real version-control tool over the
//! repository, diff, merge and worktree crates (ticket 006).
//!
//! There is no staging area: `commit` snapshots every file in the working
//! directory (except `.ghola`), like `git commit -a` plus adding new files.
//! Commands: init, commit, log, status, diff, branch, checkout, merge.
//!
//! Time enters only here: the commit timestamp is the system clock unless
//! `GHOLA_TIMESTAMP` is set (used by the tests for reproducible commits);
//! the author comes from `--author`, then `GHOLA_AUTHOR`, then "unknown".

use std::io::Write;
use std::path::{Path, PathBuf};

use diff::unified;
use object::{Object, ObjectId};
use repo::{Files, Head, MergeOutcome, Repo};
use worktree::{apply, read_files, META_DIR};

pub struct Env {
    pub timestamp: Option<u64>,
    pub author: Option<String>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            timestamp: std::env::var("GHOLA_TIMESTAMP")
                .ok()
                .and_then(|v| v.parse().ok()),
            author: std::env::var("GHOLA_AUTHOR").ok(),
        }
    }

    fn now(&self) -> u64 {
        self.timestamp.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        })
    }
}

#[derive(Debug)]
pub struct CliError(pub String);

impl<E: std::fmt::Display> From<E> for CliError {
    fn from(e: E) -> Self {
        CliError(e.to_string())
    }
}

type R<T = ()> = Result<T, CliError>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(CliError(msg.into()))
}

const USAGE: &str = "usage: ghola <command> [args]
  init                          create a repository in the current directory
  commit -m MSG [--author A]    snapshot the working directory as a new commit
  log [--oneline]               show history, newest first
  status                        show branch and uncommitted changes
  diff [REV [REV]]              show changes (working dir vs HEAD, or between revisions)
  branch [NAME]                 list branches, or create NAME at HEAD
  checkout TARGET [--force]     switch to a branch or revision
  merge REV [-m MSG] | --abort  merge a revision into the current branch
Revisions: HEAD, a branch name, a commit id or unique prefix (4+ hex digits), any of those followed by ~N.";

/// Runs one command. Returns the process exit code (0 ok, 1 command failed,
/// 2 usage error).
pub fn run(
    args: &[String],
    cwd: &Path,
    env: &Env,
    out: &mut dyn Write,
    errw: &mut dyn Write,
) -> i32 {
    let Some((cmd, rest)) = args.split_first() else {
        let _ = writeln!(errw, "{USAGE}");
        return 2;
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        let _ = writeln!(out, "{USAGE}");
        return 0;
    }
    let result = match cmd.as_str() {
        "init" => cmd_init(cwd, out),
        "commit" => Ctx::open(cwd, env).and_then(|mut c| c.commit(rest, out)),
        "log" => Ctx::open(cwd, env).and_then(|c| c.log(rest, out)),
        "status" => Ctx::open(cwd, env).and_then(|c| c.status(out)),
        "diff" => Ctx::open(cwd, env).and_then(|c| c.diff(rest, out)),
        "branch" => Ctx::open(cwd, env).and_then(|mut c| c.branch(rest, out)),
        "checkout" => Ctx::open(cwd, env).and_then(|mut c| c.checkout(rest, out)),
        "merge" => Ctx::open(cwd, env).and_then(|mut c| c.merge(rest, out)),
        other => {
            let _ = writeln!(errw, "ghola: unknown command {other:?}\n{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(CliError(msg)) => {
            let _ = writeln!(errw, "ghola: {msg}");
            1
        }
    }
}

fn find_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| d.join(META_DIR).is_dir())
        .map(Path::to_path_buf)
}

fn cmd_init(cwd: &Path, out: &mut dyn Write) -> R {
    let meta = cwd.join(META_DIR);
    if meta.exists() {
        return err(format!("already a ghola repository: {}", meta.display()));
    }
    std::fs::create_dir_all(&meta)?;
    let mut repo = Repo::open(&meta)?;
    repo.set_head(&Head::Branch("main".into()))?;
    writeln!(
        out,
        "Initialized empty ghola repository in {}",
        meta.display()
    )?;
    Ok(())
}

struct Ctx<'e> {
    root: PathBuf,
    repo: Repo,
    env: &'e Env,
}

fn opt(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn short(id: &ObjectId) -> String {
    id.to_hex()[..7].to_string()
}

impl<'e> Ctx<'e> {
    fn open(cwd: &Path, env: &'e Env) -> R<Ctx<'e>> {
        let Some(root) = find_root(cwd) else {
            return err("not a ghola repository (no .ghola found in this or any parent directory)");
        };
        let repo = Repo::open(root.join(META_DIR))?;
        Ok(Ctx { root, repo, env })
    }

    fn author(&self, args: &[String]) -> String {
        opt(args, "--author")
            .or_else(|| self.env.author.clone())
            .unwrap_or_else(|| "unknown".into())
    }

    fn tree_files(&self, commit: &ObjectId) -> R<Files> {
        Ok(self.repo.read_tree(&self.repo.commit_of(commit)?.tree)?)
    }

    fn head_files(&self) -> R<Files> {
        match self.repo.head_commit()? {
            Some(c) => self.tree_files(&c),
            None => Ok(Files::new()),
        }
    }

    /// Resolves HEAD, a branch, a commit id or unique prefix, optionally
    /// followed by `~N` (first-parent steps back).
    fn resolve(&self, spec: &str) -> R<ObjectId> {
        let (name, steps) = match spec.rsplit_once('~') {
            Some((n, s)) if !n.is_empty() => (
                n,
                s.parse::<usize>()
                    .map_err(|_| CliError(format!("bad revision {spec:?}")))?,
            ),
            _ => (spec, 0),
        };
        let mut id = if name == "HEAD" {
            self.repo
                .head_commit()?
                .ok_or_else(|| CliError("HEAD has no commits yet".into()))?
        } else if let Some(id) = self.repo.get_ref(name) {
            id
        } else if name.len() >= 4 && name.chars().all(|c| c.is_ascii_hexdigit()) {
            let lower = name.to_ascii_lowercase();
            let mut hits = self.repo.object_ids().into_iter().filter(|id| {
                id.to_hex().starts_with(&lower)
                    && matches!(self.repo.get(id), Ok(Some(Object::Commit(_))))
            });
            match (hits.next(), hits.next()) {
                (Some(id), None) => id,
                (Some(_), Some(_)) => return err(format!("ambiguous revision {spec:?}")),
                _ => return err(format!("unknown revision {spec:?}")),
            }
        } else {
            return err(format!("unknown revision {spec:?}"));
        };
        for _ in 0..steps {
            id = *self
                .repo
                .commit_of(&id)?
                .parents
                .first()
                .ok_or_else(|| CliError(format!("{spec:?}: history is not that long")))?;
        }
        Ok(id)
    }

    fn move_head_to(&mut self, id: ObjectId) -> R {
        match self.repo.head()? {
            Some(Head::Branch(b)) => self.repo.set_ref(&b, id)?,
            _ => self.repo.set_head(&Head::Detached(id))?,
        }
        Ok(())
    }

    /// Refuses to proceed if tracked files that a change from `from` to `to`
    /// would touch have local modifications.
    fn check_no_local_changes(&self, from: &Files, to: &Files) -> R {
        let now = read_files(&self.root)?;
        let mut clobbered: Vec<String> = Vec::new();
        for p in from.keys().chain(to.keys()) {
            if from.get(p) != to.get(p) && now.get(p) != from.get(p) {
                clobbered.push(String::from_utf8_lossy(p).into_owned());
            }
        }
        clobbered.sort();
        clobbered.dedup();
        if clobbered.is_empty() {
            Ok(())
        } else {
            err(format!(
                "local changes would be overwritten: {} (commit them, or use --force)",
                clobbered.join(", ")
            ))
        }
    }

    fn commit(&mut self, args: &[String], out: &mut dyn Write) -> R {
        let Some(message) = opt(args, "-m") else {
            return err("commit needs a message: ghola commit -m MSG");
        };
        let files = read_files(&self.root)?;
        let tree = self.repo.write_tree(&files)?;
        let head = self.repo.head_commit()?;
        let merge_head = self.repo.merge_head();
        if merge_head.is_none() {
            if let Some(h) = head {
                if self.repo.commit_of(&h)?.tree == tree {
                    return err("nothing to commit, working tree clean");
                }
            }
        }
        let parents: Vec<ObjectId> = head.into_iter().chain(merge_head).collect();
        let id =
            self.repo
                .write_commit(tree, parents, &self.author(args), &message, self.env.now())?;
        self.move_head_to(id)?;
        self.repo.clear_merge_head()?;
        let label = match self.repo.head()? {
            Some(Head::Branch(b)) => b,
            _ => "detached HEAD".into(),
        };
        writeln!(
            out,
            "[{label} {}] {}",
            short(&id),
            message.lines().next().unwrap_or("")
        )?;
        Ok(())
    }

    fn log(&self, args: &[String], out: &mut dyn Write) -> R {
        let Some(head) = self.repo.head_commit()? else {
            return err("no commits yet");
        };
        let oneline = args.iter().any(|a| a == "--oneline");
        for id in self.repo.log(&head)? {
            let c = self.repo.commit_of(&id)?;
            if oneline {
                writeln!(
                    out,
                    "{} {}",
                    short(&id),
                    c.message.lines().next().unwrap_or("")
                )?;
            } else {
                writeln!(out, "commit {id}")?;
                if c.parents.len() > 1 {
                    let ps: Vec<String> = c.parents.iter().map(short).collect();
                    writeln!(out, "Merge:  {}", ps.join(" "))?;
                }
                writeln!(out, "Author: {}\nDate:   {}\n", c.author, c.timestamp)?;
                for line in c.message.lines() {
                    writeln!(out, "    {line}")?;
                }
                writeln!(out)?;
            }
        }
        Ok(())
    }

    fn status(&self, out: &mut dyn Write) -> R {
        match self.repo.head()? {
            Some(Head::Branch(b)) => writeln!(out, "On branch {b}")?,
            Some(Head::Detached(id)) => writeln!(out, "HEAD detached at {}", short(&id))?,
            None => writeln!(out, "no HEAD")?,
        }
        if let Some(m) = self.repo.merge_head() {
            writeln!(
                out,
                "Merging {} (fix conflicts, then commit; or merge --abort)",
                short(&m)
            )?;
        }
        let (old, new) = (self.head_files()?, read_files(&self.root)?);
        let mut lines = Vec::new();
        for p in old.keys().chain(new.keys()) {
            let tag = match (old.get(p), new.get(p)) {
                (None, Some(_)) => "A",
                (Some(_), None) => "D",
                (Some(a), Some(b)) if a != b => "M",
                _ => continue,
            };
            lines.push(format!("{tag} {}", String::from_utf8_lossy(p)));
        }
        lines.sort_by(|a, b| a[2..].cmp(&b[2..]));
        lines.dedup();
        if lines.is_empty() {
            writeln!(out, "nothing to commit, working tree clean")?;
        }
        for l in lines {
            writeln!(out, "{l}")?;
        }
        Ok(())
    }

    fn diff(&self, args: &[String], out: &mut dyn Write) -> R {
        let (old, new) = match args {
            [] => (self.head_files()?, read_files(&self.root)?),
            [a] => (self.tree_files(&self.resolve(a)?)?, read_files(&self.root)?),
            [a, b] => (
                self.tree_files(&self.resolve(a)?)?,
                self.tree_files(&self.resolve(b)?)?,
            ),
            _ => return err("diff takes at most two revisions"),
        };
        let mut paths: Vec<&Vec<u8>> = old.keys().chain(new.keys()).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            let (a, b) = (old.get(p), new.get(p));
            if a == b {
                continue;
            }
            let name = String::from_utf8_lossy(p);
            writeln!(out, "diff --ghola a/{name} b/{name}")?;
            let (la, lb) = (
                if a.is_some() {
                    format!("a/{name}")
                } else {
                    "/dev/null".into()
                },
                if b.is_some() {
                    format!("b/{name}")
                } else {
                    "/dev/null".into()
                },
            );
            let empty = Vec::new();
            let (x, y) = (a.unwrap_or(&empty), b.unwrap_or(&empty));
            if x.contains(&0) || y.contains(&0) {
                writeln!(out, "Binary files {la} and {lb} differ")?;
            } else {
                write!(out, "{}", unified(&la, &lb, x, y, 3))?;
            }
        }
        Ok(())
    }

    fn branch(&mut self, args: &[String], out: &mut dyn Write) -> R {
        match args.first() {
            None => {
                let current = match self.repo.head()? {
                    Some(Head::Branch(b)) => Some(b),
                    _ => None,
                };
                for name in self.repo.refs().keys() {
                    let mark = if Some(name) == current.as_ref() {
                        "*"
                    } else {
                        " "
                    };
                    writeln!(out, "{mark} {name}")?;
                }
                Ok(())
            }
            Some(name) => {
                if self.repo.get_ref(name).is_some() {
                    return err(format!("branch {name:?} already exists"));
                }
                let Some(head) = self.repo.head_commit()? else {
                    return err("cannot create a branch before the first commit");
                };
                self.repo.set_ref(name, head)?;
                writeln!(out, "Created branch {name} at {}", short(&head))?;
                Ok(())
            }
        }
    }

    fn checkout(&mut self, args: &[String], out: &mut dyn Write) -> R {
        let force = args.iter().any(|a| a == "--force");
        let Some(target) = args.iter().find(|a| !a.starts_with("--")) else {
            return err("checkout needs a branch or revision");
        };
        if self.repo.merge_head().is_some() {
            return err("a merge is in progress; commit or `merge --abort` first");
        }
        let (new_head, commit) = match self.repo.get_ref(target) {
            Some(id) => (Head::Branch(target.clone()), id),
            None => {
                let id = self.resolve(target)?;
                (Head::Detached(id), id)
            }
        };
        let (from, to) = (self.head_files()?, self.tree_files(&commit)?);
        if !force {
            self.check_no_local_changes(&from, &to)?;
        }
        let now = read_files(&self.root)?;
        let base: Files = now
            .into_iter()
            .filter(|(p, _)| from.contains_key(p) || to.contains_key(p))
            .collect();
        apply(&self.root, &base, &to)?;
        self.repo.set_head(&new_head)?;
        match new_head {
            Head::Branch(b) => writeln!(out, "Switched to branch {b}")?,
            Head::Detached(id) => writeln!(out, "HEAD is now at {}", short(&id))?,
        }
        Ok(())
    }

    fn merge(&mut self, args: &[String], out: &mut dyn Write) -> R {
        if args.iter().any(|a| a == "--abort") {
            if self.repo.merge_head().is_none() {
                return err("no merge in progress");
            }
            let head = self.head_files()?;
            let theirs = self.tree_files(&self.repo.merge_head().unwrap())?;
            let now: Files = read_files(&self.root)?
                .into_iter()
                .filter(|(p, _)| head.contains_key(p) || theirs.contains_key(p))
                .collect();
            apply(&self.root, &now, &head)?;
            self.repo.clear_merge_head()?;
            writeln!(out, "Merge aborted")?;
            return Ok(());
        }
        if self.repo.merge_head().is_some() {
            return err("a merge is already in progress; commit or `merge --abort`");
        }
        let Some(rev) = args
            .iter()
            .find(|a| !a.starts_with('-') && Some(*a) != opt(args, "-m").as_ref())
        else {
            return err("merge needs a revision");
        };
        let theirs = self.resolve(rev)?;
        let Some(ours) = self.repo.head_commit()? else {
            return err("cannot merge before the first commit");
        };
        let head_files = self.head_files()?;
        let theirs_files = self.tree_files(&theirs)?;
        let now = read_files(&self.root)?;
        let dirty: Vec<String> = head_files
            .iter()
            .filter(|(p, c)| now.get(*p) != Some(*c))
            .map(|(p, _)| String::from_utf8_lossy(p).into_owned())
            .collect();
        if !dirty.is_empty() {
            return err(format!(
                "commit or discard local changes first: {}",
                dirty.join(", ")
            ));
        }
        let msg = opt(args, "-m").unwrap_or_else(|| format!("Merge {rev}"));
        match self
            .repo
            .merge_commits(&ours, &theirs, &self.author(args), &msg, self.env.now())?
        {
            MergeOutcome::AlreadyUpToDate => writeln!(out, "Already up to date.")?,
            MergeOutcome::FastForward(id) => {
                apply(&self.root, &head_files, &theirs_files)?;
                self.move_head_to(id)?;
                writeln!(out, "Fast-forward to {}", short(&id))?;
            }
            MergeOutcome::Merged(id) => {
                let merged = self.tree_files(&id)?;
                apply(&self.root, &head_files, &merged)?;
                self.move_head_to(id)?;
                writeln!(out, "Merge made: {}", short(&id))?;
            }
            MergeOutcome::Conflicts(tm) => {
                apply(&self.root, &head_files, &tm.files)?;
                self.repo.set_merge_head(&theirs)?;
                for c in &tm.conflicts {
                    writeln!(
                        out,
                        "CONFLICT ({:?}): {}",
                        c.kind,
                        String::from_utf8_lossy(&c.path)
                    )?;
                }
                return err(
                    "automatic merge failed; fix the conflicts and commit, or `merge --abort`",
                );
            }
        }
        Ok(())
    }
}
