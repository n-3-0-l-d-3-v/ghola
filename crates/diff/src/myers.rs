//! Myers' O(ND) shortest-edit-script diff ("An O(ND) Difference Algorithm and
//! Its Variations", 1986), generic over any comparable element. The result
//! is a `Patch`: a self-contained edit script (inserted elements are carried
//! inside it) that turns `a` into `b` when applied. A patch is computed on
//! demand from two snapshots and is never the stored form of history.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOp<T> {
    /// Keep the next `n` elements of the source unchanged.
    Equal(usize),
    /// Drop the next `n` elements of the source.
    Delete(usize),
    /// Emit these elements.
    Insert(Vec<T>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch<T> {
    pub ops: Vec<PatchOp<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("patch consumes {needed} source elements but the source has {available}")]
    SourceTooShort { needed: usize, available: usize },
    #[error("patch consumes {consumed} source elements but the source has {available}")]
    SourceTooLong { consumed: usize, available: usize },
}

impl<T: Clone> Patch<T> {
    /// Applies the patch to `a`. Fails (rather than guessing) if the patch
    /// does not consume exactly `a.len()` source elements.
    pub fn apply(&self, a: &[T]) -> Result<Vec<T>, PatchError> {
        let mut out = Vec::new();
        let mut pos = 0;
        for op in &self.ops {
            match op {
                PatchOp::Equal(n) | PatchOp::Delete(n) => {
                    if pos + n > a.len() {
                        return Err(PatchError::SourceTooShort {
                            needed: pos + n,
                            available: a.len(),
                        });
                    }
                    if let PatchOp::Equal(_) = op {
                        out.extend_from_slice(&a[pos..pos + n]);
                    }
                    pos += n;
                }
                PatchOp::Insert(items) => out.extend(items.iter().cloned()),
            }
        }
        if pos != a.len() {
            return Err(PatchError::SourceTooLong {
                consumed: pos,
                available: a.len(),
            });
        }
        Ok(out)
    }

    /// Number of elements deleted plus inserted: the edit distance the
    /// algorithm minimizes.
    pub fn edit_distance(&self) -> usize {
        self.ops
            .iter()
            .map(|op| match op {
                PatchOp::Equal(_) => 0,
                PatchOp::Delete(n) => *n,
                PatchOp::Insert(v) => v.len(),
            })
            .sum()
    }

    /// The patch that undoes this one (turns `b` back into `a`). Needs the
    /// original `a` to recover the deleted elements.
    pub fn invert(&self, a: &[T]) -> Result<Patch<T>, PatchError> {
        let mut ops = Vec::new();
        let mut pos = 0;
        for op in &self.ops {
            match op {
                PatchOp::Equal(n) => {
                    ops.push(PatchOp::Equal(*n));
                    pos += n;
                }
                PatchOp::Delete(n) => {
                    if pos + n > a.len() {
                        return Err(PatchError::SourceTooShort {
                            needed: pos + n,
                            available: a.len(),
                        });
                    }
                    ops.push(PatchOp::Insert(a[pos..pos + n].to_vec()));
                    pos += n;
                }
                PatchOp::Insert(items) => ops.push(PatchOp::Delete(items.len())),
            }
        }
        Ok(Patch { ops })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Keep,
    Del,
    Ins,
}

/// Computes a minimal edit script from `a` to `b`.
pub fn diff<T: Eq + Clone>(a: &[T], b: &[T]) -> Patch<T> {
    // Trimming a common prefix and suffix cannot change the edit distance and
    // shrinks the quadratic part of the work.
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a2, b2) = (&a[prefix..], &b[prefix..]);
    let suffix = a2
        .iter()
        .rev()
        .zip(b2.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (core_a, core_b) = (&a2[..a2.len() - suffix], &b2[..b2.len() - suffix]);

    let mut steps = vec![Step::Keep; prefix];
    steps.extend(myers_steps(core_a, core_b));
    steps.extend(std::iter::repeat_n(Step::Keep, suffix));

    // Coalesce steps into runs, pulling inserted elements from `b`.
    let mut ops: Vec<PatchOp<T>> = Vec::new();
    let mut bi = 0;
    for s in steps {
        match s {
            Step::Keep => {
                bi += 1;
                match ops.last_mut() {
                    Some(PatchOp::Equal(n)) => *n += 1,
                    _ => ops.push(PatchOp::Equal(1)),
                }
            }
            Step::Del => match ops.last_mut() {
                Some(PatchOp::Delete(n)) => *n += 1,
                _ => ops.push(PatchOp::Delete(1)),
            },
            Step::Ins => {
                let item = b[bi].clone();
                bi += 1;
                match ops.last_mut() {
                    Some(PatchOp::Insert(v)) => v.push(item),
                    _ => ops.push(PatchOp::Insert(vec![item])),
                }
            }
        }
    }
    Patch { ops }
}

fn myers_steps<T: Eq>(a: &[T], b: &[T]) -> Vec<Step> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 {
        return vec![Step::Ins; m as usize];
    }
    if m == 0 {
        return vec![Step::Del; n as usize];
    }
    let max = n + m;
    let off = max;
    let mut v = vec![0isize; (2 * max + 2) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = |k: isize| (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                v[idx(k + 1)]
            } else {
                v[idx(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x;
            if x >= n && y >= m {
                break 'search;
            }
            k += 2;
        }
    }

    // Walk the trace backwards to recover the path.
    let mut steps = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let idx = |k: isize| (k + off) as usize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[idx(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            steps.push(Step::Keep);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            steps.push(if x == prev_x { Step::Ins } else { Step::Del });
        }
        x = prev_x;
        y = prev_y;
    }
    steps.reverse();
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: &str, b: &str) -> Patch<char> {
        diff(
            &a.chars().collect::<Vec<_>>(),
            &b.chars().collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_papers_example_has_edit_distance_five() {
        // ABCABBA -> CBABAC is the worked example in Myers' paper: D = 5.
        let p = d("ABCABBA", "CBABAC");
        assert_eq!(p.edit_distance(), 5);
        let a: Vec<char> = "ABCABBA".chars().collect();
        assert_eq!(p.apply(&a).unwrap().iter().collect::<String>(), "CBABAC");
    }

    #[test]
    fn identical_and_empty_inputs() {
        assert_eq!(d("", "").ops, vec![]);
        assert_eq!(d("abc", "abc").ops, vec![PatchOp::Equal(3)]);
        assert_eq!(d("", "ab").ops, vec![PatchOp::Insert(vec!['a', 'b'])]);
        assert_eq!(d("ab", "").ops, vec![PatchOp::Delete(2)]);
    }

    #[test]
    fn a_single_change_in_the_middle() {
        let p = d("abcde", "abXde");
        assert_eq!(
            p.ops,
            vec![
                PatchOp::Equal(2),
                PatchOp::Delete(1),
                PatchOp::Insert(vec!['X']),
                PatchOp::Equal(2)
            ]
        );
    }

    #[test]
    fn applying_to_the_wrong_source_is_an_error() {
        let p = d("abc", "abd");
        assert!(matches!(
            p.apply(&['a']),
            Err(PatchError::SourceTooShort { .. })
        ));
        assert!(matches!(
            p.apply(&['a', 'b', 'c', 'd']),
            Err(PatchError::SourceTooLong { .. })
        ));
    }

    #[test]
    fn a_patch_can_be_inverted() {
        let (a, b): (Vec<char>, Vec<char>) =
            ("kitten".chars().collect(), "sitting".chars().collect());
        let p = diff(&a, &b);
        let back = p.invert(&a).unwrap();
        assert_eq!(back.apply(&b).unwrap(), a);
    }
}
