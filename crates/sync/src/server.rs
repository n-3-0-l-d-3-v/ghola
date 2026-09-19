//! The serving side: a pure function from `(repository, method, payload)` to
//! `(ok, response)`. It never trusts the peer: malformed payloads are errors,
//! not panics; pushed objects must be canonical valid objects; a ref only
//! moves if the new commit's whole history is present, the caller's idea of
//! the old value is current (compare-and-set), and the move is a
//! fast-forward unless forced.

use std::collections::BTreeSet;

use object::ObjectId;
use repo::Repo;

use crate::wire::{BATCH_BYTES, FETCH, LIST_REFS, PUT_OBJECTS, R, UPDATE_REF, W};

fn fail(msg: &str) -> (bool, Vec<u8>) {
    (false, msg.as_bytes().to_vec())
}

/// Objects `wants` need that `haves` do not already cover, sorted by id.
pub fn plan(repo: &Repo, wants: &[ObjectId], haves: &[ObjectId]) -> Result<Vec<ObjectId>, String> {
    let mut have_set = BTreeSet::new();
    for h in haves {
        // A have we lack, or whose history is incomplete here, just means we
        // cannot exclude anything on its account: send more, never less.
        if repo.has(h) {
            if let Ok(r) = repo.reachable(&[*h]) {
                have_set.extend(r);
            }
        }
    }
    let mut want_set = BTreeSet::new();
    for w in wants {
        let r = repo
            .reachable(&[*w])
            .map_err(|e| format!("cannot serve {w}: {e}"))?;
        want_set.extend(r);
    }
    Ok(want_set.difference(&have_set).copied().collect())
}

pub fn serve(repo: &mut Repo, method: u8, payload: &[u8]) -> (bool, Vec<u8>) {
    match method {
        LIST_REFS => {
            if !payload.is_empty() {
                return fail("LIST_REFS takes no payload");
            }
            let refs = repo.refs();
            let mut w = W::default();
            w.u32(refs.len() as u32);
            for (name, id) in &refs {
                w.string(name).id(id);
            }
            (true, w.0)
        }
        FETCH => {
            let mut r = R(payload);
            let parsed = (|| {
                let offset = r.u32()? as usize;
                let n = r.count(32)?;
                let wants: Option<Vec<ObjectId>> = (0..n).map(|_| r.id()).collect();
                let m = r.count(32)?;
                let haves: Option<Vec<ObjectId>> = (0..m).map(|_| r.id()).collect();
                r.finished().then_some((offset, wants?, haves?))
            })();
            let Some((offset, wants, haves)) = parsed else {
                return fail("malformed FETCH request");
            };
            let all = match plan(repo, &wants, &haves) {
                Ok(p) => p,
                Err(e) => return fail(&e),
            };
            let mut body = W::default();
            let (mut count, mut used) = (0u32, 0usize);
            for id in all.iter().skip(offset) {
                let Ok(bytes) = repo.raw(id) else {
                    return fail("object became unreadable");
                };
                if count > 0 && used + bytes.len() > BATCH_BYTES {
                    break;
                }
                used += bytes.len();
                count += 1;
                body.bytes(&bytes);
            }
            let mut w = W::default();
            w.u32(all.len() as u32).u32(count);
            w.0.extend(body.0);
            (true, w.0)
        }
        PUT_OBJECTS => {
            let mut r = R(payload);
            let Some(n) = r.count(4) else {
                return fail("malformed PUT_OBJECTS request");
            };
            for _ in 0..n {
                let Some(bytes) = r.bytes() else {
                    return fail("malformed PUT_OBJECTS request");
                };
                if let Err(e) = repo.put_raw(bytes) {
                    return fail(&format!("rejected object: {e}"));
                }
            }
            if !r.finished() {
                return fail("trailing bytes in PUT_OBJECTS request");
            }
            let mut w = W::default();
            w.u32(n as u32);
            (true, w.0)
        }
        UPDATE_REF => {
            let mut r = R(payload);
            let parsed = (|| {
                let name = r.string()?;
                let old = match r.u8()? {
                    0 => None,
                    1 => Some(r.id()?),
                    _ => return None,
                };
                let new = r.id()?;
                let force = r.u8()? == 1;
                r.finished().then_some((name, old, new, force))
            })();
            let Some((name, old, new, force)) = parsed else {
                return fail("malformed UPDATE_REF request");
            };
            let current = repo.get_ref(&name);
            if current != old {
                return fail("stale: the remote ref is not at the value you based this on");
            }
            if !repo.is_complete(&new) {
                return fail("incomplete: the new commit's history is not fully present");
            }
            if let (Some(cur), false) = (current, force) {
                match repo.is_ancestor(&cur, &new) {
                    Ok(true) => {}
                    Ok(false) => return fail("non-fast-forward: the remote has commits you lack"),
                    Err(e) => return fail(&format!("cannot compare histories: {e}")),
                }
            }
            match repo.set_ref(&name, new) {
                Ok(()) => (true, Vec::new()),
                Err(e) => fail(&format!("cannot update ref: {e}")),
            }
        }
        other => fail(&format!("unknown method {other}")),
    }
}
