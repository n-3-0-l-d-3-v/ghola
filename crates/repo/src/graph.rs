//! History as a DAG: commit creation, ancestry, topological log order and
//! merge bases (ticket 003). Every query walks parent links through the
//! verified object store; nothing is cached, so nothing can go stale.

use std::collections::{BTreeSet, BinaryHeap, HashMap};

use object::{Commit, Object, ObjectId};

use crate::{Repo, RepoError};

impl Repo {
    /// Stores a commit object and returns its id.
    pub fn write_commit(
        &mut self,
        tree: ObjectId,
        parents: Vec<ObjectId>,
        author: &str,
        message: &str,
        timestamp: u64,
    ) -> Result<ObjectId, RepoError> {
        self.put(&Object::Commit(Commit {
            tree,
            parents,
            timestamp,
            author: author.to_string(),
            message: message.to_string(),
        }))
    }

    /// `id` and everything reachable from it through parent links.
    pub fn ancestors(&self, id: &ObjectId) -> Result<BTreeSet<ObjectId>, RepoError> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![*id];
        while let Some(c) = stack.pop() {
            if seen.insert(c) {
                stack.extend(self.commit_of(&c)?.parents);
            }
        }
        Ok(seen)
    }

    /// True if `ancestor` is reachable from `descendant` (a commit is its own
    /// ancestor).
    pub fn is_ancestor(
        &self,
        ancestor: &ObjectId,
        descendant: &ObjectId,
    ) -> Result<bool, RepoError> {
        Ok(self.ancestors(descendant)?.contains(ancestor))
    }

    /// Every commit reachable from `from`, in a topological order: a commit
    /// always appears before all of its parents. Ties (independent branches)
    /// are broken by newest timestamp first, then by id, so the order is
    /// deterministic. Timestamps are only a tie-break; correctness never
    /// depends on them.
    pub fn log(&self, from: &ObjectId) -> Result<Vec<ObjectId>, RepoError> {
        let nodes = self.ancestors(from)?;
        let mut commits: HashMap<ObjectId, Commit> = HashMap::new();
        for id in &nodes {
            commits.insert(*id, self.commit_of(id)?);
        }
        // Number of not-yet-emitted children each commit still has.
        let mut pending: HashMap<ObjectId, usize> = nodes.iter().map(|n| (*n, 0)).collect();
        for c in commits.values() {
            for p in &c.parents {
                *pending.get_mut(p).expect("parent is an ancestor") += 1;
            }
        }
        let mut ready: BinaryHeap<(u64, ObjectId)> = BinaryHeap::new();
        ready.push((commits[from].timestamp, *from));
        let mut out = Vec::with_capacity(nodes.len());
        while let Some((_, id)) = ready.pop() {
            out.push(id);
            for p in &commits[&id].parents {
                let n = pending.get_mut(p).unwrap();
                *n -= 1;
                if *n == 0 {
                    ready.push((commits[p].timestamp, *p));
                }
            }
        }
        debug_assert_eq!(out.len(), nodes.len(), "a DAG has no cycles");
        Ok(out)
    }

    /// The mainline: `from`, its first parent, that commit's first parent, ...
    pub fn first_parent_chain(&self, from: &ObjectId) -> Result<Vec<ObjectId>, RepoError> {
        let mut out = vec![*from];
        let mut cur = self.commit_of(from)?;
        while let Some(p) = cur.parents.first() {
            out.push(*p);
            cur = self.commit_of(p)?;
        }
        Ok(out)
    }

    /// The best common ancestors of `a` and `b`: common ancestors that are
    /// not themselves ancestors of another common ancestor. Usually one;
    /// two or more after a criss-cross merge. Sorted by id.
    pub fn merge_bases(&self, a: &ObjectId, b: &ObjectId) -> Result<Vec<ObjectId>, RepoError> {
        let common: BTreeSet<ObjectId> = self
            .ancestors(a)?
            .intersection(&self.ancestors(b)?)
            .copied()
            .collect();
        // Everything reachable from a proper parent of a common ancestor is
        // dominated by that ancestor.
        let mut dominated = BTreeSet::new();
        let mut stack: Vec<ObjectId> = Vec::new();
        for c in &common {
            stack.extend(self.commit_of(c)?.parents);
        }
        while let Some(x) = stack.pop() {
            if dominated.insert(x) {
                stack.extend(self.commit_of(&x)?.parents);
            }
        }
        Ok(common.difference(&dominated).copied().collect())
    }

    /// One merge base, chosen deterministically (newest timestamp, then id);
    /// `None` if the histories are unrelated.
    pub fn merge_base(&self, a: &ObjectId, b: &ObjectId) -> Result<Option<ObjectId>, RepoError> {
        let mut best: Option<(u64, ObjectId)> = None;
        for id in self.merge_bases(a, b)? {
            let key = (self.commit_of(&id)?.timestamp, id);
            if best.is_none_or(|cur| key > cur) {
                best = Some(key);
            }
        }
        Ok(best.map(|(_, id)| id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct H {
        _dir: tempfile::TempDir,
        repo: Repo,
        tree: ObjectId,
        clock: u64,
    }

    impl H {
        fn new() -> H {
            let dir = tempfile::tempdir().unwrap();
            let mut repo = Repo::open(dir.path()).unwrap();
            let tree = repo.write_tree(&Default::default()).unwrap();
            H {
                _dir: dir,
                repo,
                tree,
                clock: 0,
            }
        }
        fn commit(&mut self, name: &str, parents: &[ObjectId]) -> ObjectId {
            self.clock += 1;
            self.repo
                .write_commit(self.tree, parents.to_vec(), "t", name, self.clock)
                .unwrap()
        }
    }

    #[test]
    fn linear_history() {
        let mut h = H::new();
        let a = h.commit("a", &[]);
        let b = h.commit("b", &[a]);
        let c = h.commit("c", &[b]);
        assert_eq!(h.repo.log(&c).unwrap(), vec![c, b, a]);
        assert_eq!(h.repo.first_parent_chain(&c).unwrap(), vec![c, b, a]);
        assert!(h.repo.is_ancestor(&a, &c).unwrap());
        assert!(!h.repo.is_ancestor(&c, &a).unwrap());
        assert!(
            h.repo.is_ancestor(&b, &b).unwrap(),
            "a commit is its own ancestor"
        );
        assert_eq!(h.repo.merge_bases(&a, &c).unwrap(), vec![a]);
    }

    #[test]
    fn a_diamond_merge_has_the_fork_point_as_base_and_logs_topologically() {
        let mut h = H::new();
        let root = h.commit("root", &[]);
        let left = h.commit("left", &[root]);
        let right = h.commit("right", &[root]);
        let merge = h.commit("merge", &[left, right]);
        assert_eq!(h.repo.merge_bases(&left, &right).unwrap(), vec![root]);
        assert_eq!(h.repo.merge_base(&left, &right).unwrap(), Some(root));
        let log = h.repo.log(&merge).unwrap();
        assert_eq!(
            log,
            vec![merge, right, left, root],
            "newest independent branch first, root last"
        );
        assert_eq!(
            h.repo.first_parent_chain(&merge).unwrap(),
            vec![merge, left, root]
        );
    }

    #[test]
    fn a_criss_cross_merge_has_two_best_common_ancestors() {
        let mut h = H::new();
        let root = h.commit("root", &[]);
        let a = h.commit("a", &[root]);
        let b = h.commit("b", &[root]);
        let a2 = h.commit("a2", &[a, b]);
        let b2 = h.commit("b2", &[b, a]);
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(h.repo.merge_bases(&a2, &b2).unwrap(), want);
        assert!(h.repo.merge_base(&a2, &b2).unwrap().is_some());
    }

    #[test]
    fn unrelated_histories_have_no_merge_base() {
        let mut h = H::new();
        let a = h.commit("a", &[]);
        let b = h.commit("b", &[]);
        assert!(h.repo.merge_bases(&a, &b).unwrap().is_empty());
        assert_eq!(h.repo.merge_base(&a, &b).unwrap(), None);
    }

    #[test]
    fn when_one_side_is_an_ancestor_it_is_the_merge_base() {
        let mut h = H::new();
        let a = h.commit("a", &[]);
        let b = h.commit("b", &[a]);
        assert_eq!(h.repo.merge_bases(&a, &b).unwrap(), vec![a]);
        assert_eq!(h.repo.merge_bases(&b, &a).unwrap(), vec![a]);
        assert_eq!(h.repo.merge_bases(&b, &b).unwrap(), vec![b]);
    }

    #[test]
    fn log_order_never_trusts_timestamps() {
        // The parent has a newer timestamp than its child (clock skew).
        let mut h = H::new();
        let parent = h
            .repo
            .write_commit(h.tree, vec![], "t", "parent", 1000)
            .unwrap();
        let child = h
            .repo
            .write_commit(h.tree, vec![parent], "t", "child", 5)
            .unwrap();
        assert_eq!(h.repo.log(&child).unwrap(), vec![child, parent]);
    }

    #[test]
    fn a_missing_parent_is_an_error_not_a_silent_truncation() {
        let mut h = H::new();
        let ghost = ObjectId::of(b"ghost");
        let c = h.commit("orphan", &[ghost]);
        assert!(matches!(h.repo.log(&c), Err(RepoError::Missing(_))));
        assert!(h.repo.ancestors(&c).is_err());
    }
}
