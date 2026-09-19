//! `fetch`, `push` and `pull`: sync with another ghola repository on disk,
//! carried over distrans's simulated network (optionally hostile).

use std::io::Write;

use repo::{Files, Head, MergeOutcome, Repo};
use sync::{DistransRemote, NetConfig};
use worktree::{apply, read_files, META_DIR};

use crate::{err, opt, short, CliError, Ctx, R};

/// Arguments that are not flags and not the value of a flag, in order.
fn positionals(args: &[String]) -> Vec<&String> {
    args.iter()
        .enumerate()
        .filter(|(i, a)| {
            !a.starts_with("--")
                && !(*i > 0
                    && matches!(
                        args[*i - 1].as_str(),
                        "--name" | "--loss" | "--seed" | "--author"
                    ))
        })
        .map(|(_, a)| a)
        .collect()
}

impl Ctx<'_> {
    fn open_remote(&self, args: &[String]) -> R<(Repo, NetConfig)> {
        let Some(path) = positionals(args).first().map(|s| s.to_string()) else {
            return err("this command needs the path of another ghola repository");
        };
        let meta = self.cwd.join(&path).join(META_DIR);
        if !meta.is_dir() {
            return err(format!("{path:?} is not a ghola repository"));
        }
        let loss: f64 = match opt(args, "--loss") {
            Some(v) => v
                .parse()
                .map_err(|_| CliError(format!("bad --loss {v:?}")))?,
            None => 0.0,
        };
        if !(0.0..1.0).contains(&loss) {
            return err("--loss must be at least 0 and below 1");
        }
        let seed: u64 = match opt(args, "--seed") {
            Some(v) => v
                .parse()
                .map_err(|_| CliError(format!("bad --seed {v:?}")))?,
            None => 1,
        };
        let cfg = if loss > 0.0 {
            NetConfig::hostile(seed, loss)
        } else {
            NetConfig::clean(seed)
        };
        Ok((Repo::open(meta)?, cfg))
    }

    fn current_branch(&self) -> R<String> {
        match self.repo.head()? {
            Some(Head::Branch(b)) => Ok(b),
            _ => err("HEAD is detached; name a branch explicitly"),
        }
    }

    pub(crate) fn fetch(&mut self, args: &[String], out: &mut dyn Write) -> R {
        let (mut remote_repo, cfg) = self.open_remote(args)?;
        let name = opt(args, "--name").unwrap_or_else(|| "origin".into());
        let mut net = DistransRemote::new(&mut remote_repo, cfg);
        let report = sync::fetch(&mut self.repo, &mut net, &name)?;
        let s = net.stats();
        writeln!(
            out,
            "Fetched {} objects ({} bytes) in {} ticks over {} datagrams sent ({} dropped)",
            report.objects,
            report.bytes,
            s.ticks,
            s.client_to_server.sent + s.server_to_client.sent,
            s.client_to_server.dropped + s.server_to_client.dropped
        )?;
        for (r, id) in report.updated {
            writeln!(out, "  {r} -> {}", short(&id))?;
        }
        Ok(())
    }

    pub(crate) fn push(&mut self, args: &[String], out: &mut dyn Write) -> R {
        let (mut remote_repo, cfg) = self.open_remote(args)?;
        let branch = match positionals(args).get(1) {
            Some(b) => b.to_string(),
            None => self.current_branch()?,
        };
        let force = args.iter().any(|a| a == "--force");
        let mut net = DistransRemote::new(&mut remote_repo, cfg);
        let report = sync::push(&mut self.repo, &mut net, &branch, force)?;
        let s = net.stats();
        writeln!(
            out,
            "Pushed {branch}: {} objects ({} bytes) in {} ticks over {} datagrams sent ({} dropped)",
            report.objects,
            report.bytes,
            s.ticks,
            s.client_to_server.sent + s.server_to_client.sent,
            s.client_to_server.dropped + s.server_to_client.dropped
        )?;
        Ok(())
    }

    pub(crate) fn pull(&mut self, args: &[String], out: &mut dyn Write) -> R {
        let (mut remote_repo, cfg) = self.open_remote(args)?;
        let branch = match positionals(args).get(1) {
            Some(b) => b.to_string(),
            None => self.current_branch()?,
        };
        let on_branch = matches!(self.repo.head()?, Some(Head::Branch(b)) if b == branch);
        let before = if on_branch {
            self.head_files()?
        } else {
            Files::new()
        };
        if on_branch {
            let now = read_files(&self.root)?;
            let dirty: Vec<String> = before
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
        }
        let author = self.author(args);
        let timestamp = self.env.now();
        let mut net = DistransRemote::new(&mut remote_repo, cfg);
        let outcome = sync::pull(
            &mut self.repo,
            &mut net,
            "origin",
            &branch,
            &author,
            timestamp,
        )?;
        match outcome {
            MergeOutcome::AlreadyUpToDate => writeln!(out, "Already up to date.")?,
            MergeOutcome::FastForward(id) | MergeOutcome::Merged(id) => {
                if on_branch {
                    let after = self.tree_files(&id)?;
                    apply(&self.root, &before, &after)?;
                }
                writeln!(out, "{branch} is now at {}", short(&id))?;
            }
            MergeOutcome::Conflicts(tm) => {
                for c in &tm.conflicts {
                    writeln!(
                        out,
                        "CONFLICT ({:?}): {}",
                        c.kind,
                        String::from_utf8_lossy(&c.path)
                    )?;
                }
                return err(format!(
                    "the histories conflict; run `ghola merge remotes/origin/{branch}` to resolve them here"
                ));
            }
        }
        Ok(())
    }
}
