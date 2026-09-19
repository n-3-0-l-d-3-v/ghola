//! Property tests (ticket 003): on random DAGs, the graph queries agree with
//! brute-force reference implementations written straight from the
//! definitions.

use std::collections::BTreeSet;

use object::ObjectId;
use proptest::prelude::*;
use repo::Repo;

/// A random DAG: commit `i` lists distinct parents among commits `0..i`, and
/// carries an arbitrary (possibly nonsensical) timestamp.
fn arb_dag() -> impl Strategy<Value = Vec<(Vec<usize>, u64)>> {
    prop::collection::vec(
        (
            prop::collection::vec(any::<prop::sample::Index>(), 0..4),
            0u64..30,
        ),
        1..24,
    )
    .prop_map(|raw| {
        raw.into_iter()
            .enumerate()
            .map(|(i, (idxs, ts))| {
                let mut parents: Vec<usize> = Vec::new();
                if i > 0 {
                    for ix in idxs {
                        let p = ix.index(i);
                        if !parents.contains(&p) {
                            parents.push(p);
                        }
                    }
                }
                (parents, ts)
            })
            .collect()
    })
}

struct Built {
    _dir: tempfile::TempDir,
    repo: Repo,
    ids: Vec<ObjectId>,
}

fn build(dag: &[(Vec<usize>, u64)]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let mut repo = Repo::open(dir.path()).unwrap();
    let tree = repo.write_tree(&Default::default()).unwrap();
    let mut ids: Vec<ObjectId> = Vec::new();
    for (i, (parents, ts)) in dag.iter().enumerate() {
        let ps = parents.iter().map(|&p| ids[p]).collect();
        ids.push(
            repo.write_commit(tree, ps, "t", &format!("c{i}"), *ts)
                .unwrap(),
        );
    }
    Built {
        _dir: dir,
        repo,
        ids,
    }
}

fn ref_ancestors(dag: &[(Vec<usize>, u64)], i: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![i];
    while let Some(c) = stack.pop() {
        if seen.insert(c) {
            stack.extend(dag[c].0.iter().copied());
        }
    }
    seen
}

fn ref_merge_bases(dag: &[(Vec<usize>, u64)], a: usize, b: usize) -> BTreeSet<usize> {
    let common: BTreeSet<usize> = ref_ancestors(dag, a)
        .intersection(&ref_ancestors(dag, b))
        .copied()
        .collect();
    common
        .iter()
        .copied()
        .filter(|&c| {
            !common
                .iter()
                .any(|&d| d != c && ref_ancestors(dag, d).contains(&c))
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn ancestors_and_is_ancestor_match_the_definition(dag in arb_dag(), pick in any::<prop::sample::Index>()) {
        let b = build(&dag);
        let i = pick.index(dag.len());
        let want: BTreeSet<ObjectId> = ref_ancestors(&dag, i).into_iter().map(|k| b.ids[k]).collect();
        prop_assert_eq!(b.repo.ancestors(&b.ids[i]).unwrap(), want);
        for j in 0..dag.len() {
            prop_assert_eq!(
                b.repo.is_ancestor(&b.ids[j], &b.ids[i]).unwrap(),
                ref_ancestors(&dag, i).contains(&j)
            );
        }
    }

    #[test]
    fn log_is_a_topological_order_of_exactly_the_ancestors(dag in arb_dag(), pick in any::<prop::sample::Index>()) {
        let b = build(&dag);
        let i = pick.index(dag.len());
        let log = b.repo.log(&b.ids[i]).unwrap();
        let expected: BTreeSet<ObjectId> = ref_ancestors(&dag, i).into_iter().map(|k| b.ids[k]).collect();
        prop_assert_eq!(log.iter().copied().collect::<BTreeSet<_>>(), expected);
        prop_assert_eq!(log.len(), ref_ancestors(&dag, i).len(), "no commit listed twice");
        prop_assert_eq!(log[0], b.ids[i]);
        let pos = |id: &ObjectId| log.iter().position(|x| x == id).unwrap();
        for k in ref_ancestors(&dag, i) {
            for &p in &dag[k].0 {
                prop_assert!(pos(&b.ids[k]) < pos(&b.ids[p]), "commit {} must precede its parent {}", k, p);
            }
        }
    }

    #[test]
    fn merge_bases_match_the_definition_and_are_symmetric(
        dag in arb_dag(),
        x in any::<prop::sample::Index>(),
        y in any::<prop::sample::Index>(),
    ) {
        let b = build(&dag);
        let (i, j) = (x.index(dag.len()), y.index(dag.len()));
        let want: BTreeSet<ObjectId> = ref_merge_bases(&dag, i, j).into_iter().map(|k| b.ids[k]).collect();
        let got: BTreeSet<ObjectId> = b.repo.merge_bases(&b.ids[i], &b.ids[j]).unwrap().into_iter().collect();
        prop_assert_eq!(&got, &want);
        let swapped: BTreeSet<ObjectId> = b.repo.merge_bases(&b.ids[j], &b.ids[i]).unwrap().into_iter().collect();
        prop_assert_eq!(&got, &swapped);
        let one = b.repo.merge_base(&b.ids[i], &b.ids[j]).unwrap();
        prop_assert_eq!(one.is_some(), !want.is_empty());
        if let Some(m) = one {
            prop_assert!(got.contains(&m));
        }
    }

    #[test]
    fn if_one_side_is_an_ancestor_it_is_the_only_merge_base(dag in arb_dag(), x in any::<prop::sample::Index>()) {
        let b = build(&dag);
        let i = x.index(dag.len());
        for j in ref_ancestors(&dag, i) {
            prop_assert_eq!(
                b.repo.merge_bases(&b.ids[j], &b.ids[i]).unwrap(),
                vec![b.ids[j]]
            );
        }
    }
}
