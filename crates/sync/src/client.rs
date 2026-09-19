//! The client side: `fetch`, `push` and `pull` over any `Remote`. A remote
//! is untrusted. Everything received is validated as a canonical object and
//! stored under the id *we compute*, and refs only move once the complete
//! history behind them is verifiably present, so a lying, truncating or
//! corrupting peer can waste time but cannot leave a ref pointing at
//! something missing.

use std::collections::{BTreeMap, BTreeSet};

use object::ObjectId;
use repo::{MergeOutcome, Repo, RepoError};

use crate::server::serve;
use crate::wire::{BATCH_BYTES, FETCH, LIST_REFS, PUT_OBJECTS, R, UPDATE_REF, W};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The remote answered, but refused.
    #[error("remote refused: {0}")]
    Remote(String),
    /// The network gave up (timeout or retries exhausted).
    #[error("network failure: {0}")]
    Transport(String),
    #[error("protocol error: {0}")]
    Protocol(&'static str),
    #[error("the remote did not deliver the complete history of {0}")]
    Incomplete(ObjectId),
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// A peer we can send requests to and get responses from.
pub trait Remote {
    fn call(&mut self, method: u8, payload: &[u8]) -> Result<(bool, Vec<u8>), SyncError>;
}

/// A remote that is just another repository in this process (no network).
pub struct Direct<'a>(pub &'a mut Repo);

impl Remote for Direct<'_> {
    fn call(&mut self, method: u8, payload: &[u8]) -> Result<(bool, Vec<u8>), SyncError> {
        Ok(serve(self.0, method, payload))
    }
}

fn ok_call(remote: &mut dyn Remote, method: u8, payload: &[u8]) -> Result<Vec<u8>, SyncError> {
    let (ok, body) = remote.call(method, payload)?;
    if ok {
        Ok(body)
    } else {
        Err(SyncError::Remote(
            String::from_utf8_lossy(&body).into_owned(),
        ))
    }
}

pub fn list_refs(remote: &mut dyn Remote) -> Result<BTreeMap<String, ObjectId>, SyncError> {
    let body = ok_call(remote, LIST_REFS, &[])?;
    let mut r = R(&body);
    let bad = SyncError::Protocol("malformed LIST_REFS response");
    let n = r
        .count(34)
        .ok_or(SyncError::Protocol("malformed LIST_REFS response"))?;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let name = r
            .string()
            .ok_or(SyncError::Protocol("malformed LIST_REFS response"))?;
        let id = r
            .id()
            .ok_or(SyncError::Protocol("malformed LIST_REFS response"))?;
        out.insert(name, id);
    }
    if !r.finished() {
        return Err(bad);
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchReport {
    pub objects: usize,
    pub bytes: usize,
    /// Remote-tracking refs created or moved.
    pub updated: Vec<(String, ObjectId)>,
}

/// Downloads every object reachable from the remote's refs that we lack,
/// then points `remotes/<remote_name>/<branch>` at each remote ref.
pub fn fetch(
    local: &mut Repo,
    remote: &mut dyn Remote,
    remote_name: &str,
) -> Result<FetchReport, SyncError> {
    let refs = list_refs(remote)?;
    let wants: Vec<ObjectId> = refs
        .values()
        .filter(|id| !local.is_complete(id))
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut report = FetchReport::default();
    if !wants.is_empty() {
        let haves: Vec<ObjectId> = local.refs().values().copied().collect();
        let mut offset: u32 = 0;
        loop {
            let mut w = W::default();
            w.u32(offset).u32(wants.len() as u32);
            wants.iter().for_each(|id| {
                w.id(id);
            });
            w.u32(haves.len() as u32);
            haves.iter().for_each(|id| {
                w.id(id);
            });
            let body = ok_call(remote, FETCH, &w.0)?;
            let mut r = R(&body);
            let (Some(total), Some(count)) = (r.u32(), r.u32()) else {
                return Err(SyncError::Protocol("malformed FETCH response"));
            };
            if count == 0 && offset < total {
                return Err(SyncError::Protocol("FETCH made no progress"));
            }
            for _ in 0..count {
                let bytes = r
                    .bytes()
                    .ok_or(SyncError::Protocol("malformed FETCH response"))?;
                local.put_raw(bytes)?;
                report.objects += 1;
                report.bytes += bytes.len();
            }
            offset = offset.saturating_add(count);
            if offset >= total {
                break;
            }
        }
        for id in &wants {
            if !local.is_complete(id) {
                return Err(SyncError::Incomplete(*id));
            }
        }
    }
    for (name, id) in refs {
        let tracking = format!("remotes/{remote_name}/{name}");
        if local.get_ref(&tracking) != Some(id) {
            local.set_ref(&tracking, id)?;
            report.updated.push((tracking, id));
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushReport {
    pub objects: usize,
    pub bytes: usize,
}

/// Uploads the objects the remote lacks for local branch `branch`, then
/// asks the remote to move its ref. The remote enforces fast-forward and
/// compare-and-set; `force` overrides the fast-forward rule only.
pub fn push(
    local: &mut Repo,
    remote: &mut dyn Remote,
    branch: &str,
    force: bool,
) -> Result<PushReport, SyncError> {
    let tip = local
        .get_ref(branch)
        .ok_or(SyncError::Protocol("no such local branch"))?;
    let remote_refs = list_refs(remote)?;
    let old = remote_refs.get(branch).copied();
    let exclude = match old {
        Some(o) if local.has(&o) => local.reachable(&[o]).unwrap_or_default(),
        _ => BTreeSet::new(),
    };
    let to_send: Vec<ObjectId> = local
        .reachable(&[tip])?
        .difference(&exclude)
        .copied()
        .collect();

    let mut report = PushReport::default();
    let mut batch: Vec<Vec<u8>> = Vec::new();
    let mut batch_bytes = 0;
    let flush = |remote: &mut dyn Remote, batch: &mut Vec<Vec<u8>>, bytes: &mut usize| {
        if batch.is_empty() {
            return Ok(());
        }
        let mut w = W::default();
        w.u32(batch.len() as u32);
        batch.iter().for_each(|b| {
            w.bytes(b);
        });
        ok_call(remote, PUT_OBJECTS, &w.0)?;
        batch.clear();
        *bytes = 0;
        Ok::<(), SyncError>(())
    };
    for id in &to_send {
        let bytes = local.raw(id)?;
        if !batch.is_empty() && batch_bytes + bytes.len() > BATCH_BYTES {
            flush(remote, &mut batch, &mut batch_bytes)?;
        }
        report.objects += 1;
        report.bytes += bytes.len();
        batch_bytes += bytes.len();
        batch.push(bytes);
    }
    flush(remote, &mut batch, &mut batch_bytes)?;

    let mut w = W::default();
    w.string(branch);
    match old {
        Some(o) => w.u8(1).id(&o),
        None => w.u8(0),
    };
    w.id(&tip).u8(u8::from(force));
    ok_call(remote, UPDATE_REF, &w.0)?;
    Ok(report)
}

/// Fetches, then folds the remote's `branch` into the local `branch`
/// (creating it if absent). Only refs move; the caller updates any working
/// directory. Conflicts are returned, not applied.
pub fn pull(
    local: &mut Repo,
    remote: &mut dyn Remote,
    remote_name: &str,
    branch: &str,
    author: &str,
    timestamp: u64,
) -> Result<MergeOutcome, SyncError> {
    fetch(local, remote, remote_name)?;
    let tracking = format!("remotes/{remote_name}/{branch}");
    let theirs = local
        .get_ref(&tracking)
        .ok_or(SyncError::Protocol("the remote has no such branch"))?;
    let Some(ours) = local.get_ref(branch) else {
        local.set_ref(branch, theirs)?;
        return Ok(MergeOutcome::FastForward(theirs));
    };
    let outcome = local.merge_commits(
        &ours,
        &theirs,
        author,
        &format!("Merge {tracking}"),
        timestamp,
    )?;
    match &outcome {
        MergeOutcome::FastForward(id) | MergeOutcome::Merged(id) => local.set_ref(branch, *id)?,
        MergeOutcome::AlreadyUpToDate | MergeOutcome::Conflicts(_) => {}
    }
    Ok(outcome)
}
