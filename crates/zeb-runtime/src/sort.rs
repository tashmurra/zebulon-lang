//! Resumable T3 quicksort for `Vector.sort` with a source comparator.
//!
//! Generated code is never called from the runtime, so a sort suspends at each
//! comparison: generated code invokes the comparator and resumes with its result.
//! Comparison and exchange order follow the reference VM's `CVmQSortData::sort`
//! exactly (an unstable quicksort), so tie order and comparator side effects match.
//! Sub-ranges are kept on an explicit stack instead of recursing; the left range
//! is always finished before the right one, as in the reference recursion.

/// Zero-based element exchange on the sorted collection.
pub(crate) trait Exchange {
    type Error;
    fn exchange(&mut self, a: usize, b: usize) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AdvanceI,
    CompareI,
    AdvanceJ,
    CompareJ,
    Exchange,
}

#[derive(Clone, Copy, Debug)]
struct Partition {
    l: i64,
    r: i64,
    i: i64,
    j: i64,
    pivot: i64,
    phase: Phase,
}

pub(crate) struct QuickSort {
    descending: bool,
    ranges: Vec<(i64, i64)>,
    partition: Option<Partition>,
}

impl QuickSort {
    /// Pending ranges are disjoint and non-empty, so `len` entries always suffice;
    /// the stack is reserved up front and never grows during the sort.
    pub(crate) fn new(len: usize, descending: bool) -> Option<Self> {
        let mut ranges = Vec::new();
        ranges.try_reserve_exact(len.max(1)).ok()?;
        if len != 0 {
            ranges.push((0, i64::try_from(len - 1).ok()?));
        }
        Some(Self {
            descending,
            ranges,
            partition: None,
        })
    }

    /// True when the last step returned a pair whose comparison is outstanding.
    pub(crate) fn awaiting(&self) -> bool {
        self.partition
            .is_some_and(|p| matches!(p.phase, Phase::CompareI | Phase::CompareJ))
    }

    /// Continue with the comparator result for the outstanding pair (required
    /// exactly when `awaiting`). Returns the next `(element, pivot)` pair to compare,
    /// or `None` when sorted.
    pub(crate) fn resume<E>(
        &mut self,
        elements: &mut impl Exchange<Error = E>,
        result: Option<i32>,
    ) -> Result<Option<(usize, usize)>, E> {
        let mut order = result.map(|n| {
            if self.descending {
                -i64::from(n)
            } else {
                i64::from(n)
            }
        });
        loop {
            let Some(mut p) = self.partition else {
                let Some((l, r)) = self.ranges.pop() else {
                    return Ok(None);
                };
                if r > l {
                    self.partition = Some(Partition {
                        l,
                        r,
                        i: l - 1,
                        j: r,
                        pivot: r,
                        phase: Phase::AdvanceI,
                    });
                }
                continue;
            };
            match p.phase {
                Phase::AdvanceI => {
                    p.i += 1;
                    if p.i != p.r {
                        p.phase = Phase::CompareI;
                        self.partition = Some(p);
                        return Ok(Some((p.i as usize, p.pivot as usize)));
                    }
                    p.phase = Phase::AdvanceJ;
                }
                Phase::CompareI => {
                    p.phase = if order.take().unwrap_or(0) < 0 {
                        Phase::AdvanceI
                    } else {
                        Phase::AdvanceJ
                    };
                }
                Phase::AdvanceJ => {
                    p.j -= 1;
                    if p.j != p.l {
                        p.phase = Phase::CompareJ;
                        self.partition = Some(p);
                        return Ok(Some((p.j as usize, p.pivot as usize)));
                    }
                    p.phase = Phase::Exchange;
                }
                Phase::CompareJ => {
                    p.phase = if order.take().unwrap_or(0) > 0 {
                        Phase::AdvanceJ
                    } else {
                        Phase::Exchange
                    };
                }
                Phase::Exchange => {
                    elements.exchange(p.i as usize, p.j as usize)?;
                    if p.pivot == p.i {
                        p.pivot = p.j;
                    } else if p.pivot == p.j {
                        p.pivot = p.i;
                    }
                    if p.j > p.i {
                        p.phase = Phase::AdvanceI;
                    } else {
                        elements.exchange(p.i as usize, p.j as usize)?;
                        elements.exchange(p.i as usize, p.r as usize)?;
                        if p.i < p.r {
                            self.ranges.push((p.i + 1, p.r));
                        }
                        if p.i > p.l {
                            self.ranges.push((p.l, p.i - 1));
                        }
                        self.partition = None;
                        continue;
                    }
                }
            }
            self.partition = Some(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Exchange for Vec<(i32, usize)> {
        type Error = ();
        fn exchange(&mut self, a: usize, b: usize) -> Result<(), ()> {
            self.swap(a, b);
            Ok(())
        }
    }

    /// Direct transliteration of the reference recursion, for comparison.
    fn reference(
        items: &mut Vec<(i32, usize)>,
        l: usize,
        r: usize,
        descending: bool,
        log: &mut Vec<(usize, usize)>,
    ) {
        let mut compare = |items: &Vec<(i32, usize)>, a: usize, b: usize| {
            log.push((items[a].1, items[b].1));
            let n = items[a].0.cmp(&items[b].0) as i32;
            if descending { -n } else { n }
        };
        if r > l {
            let mut v_idx = r;
            let mut i = l as i64 - 1;
            let mut j = r;
            loop {
                loop {
                    i += 1;
                    if !(i as usize != r && compare(items, i as usize, v_idx) < 0) {
                        break;
                    }
                }
                loop {
                    j -= 1;
                    if !(j != l && compare(items, j, v_idx) > 0) {
                        break;
                    }
                }
                let iu = i as usize;
                items.swap(iu, j);
                if v_idx == iu {
                    v_idx = j;
                } else if v_idx == j {
                    v_idx = iu;
                }
                if j <= iu {
                    break;
                }
            }
            let iu = i as usize;
            items.swap(iu, j);
            items.swap(iu, r);
            if iu > l {
                reference(items, l, iu - 1, descending, log);
            }
            if iu < r {
                reference(items, iu + 1, r, descending, log);
            }
        }
    }

    #[test]
    fn resumable_sort_matches_reference_order_and_comparisons() {
        let mut seed = 12345u32;
        for len in 0..40usize {
            for descending in [false, true] {
                let items: Vec<(i32, usize)> = (0..len)
                    .map(|id| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        (((seed >> 16) % 5) as i32, id)
                    })
                    .collect();
                let mut expected = items.clone();
                let mut expected_log = Vec::new();
                if len != 0 {
                    reference(&mut expected, 0, len - 1, descending, &mut expected_log);
                }
                let mut actual = items;
                let mut log = Vec::new();
                let mut sort = QuickSort::new(len, descending).unwrap();
                let mut result = None;
                while let Some((a, b)) = sort.resume(&mut actual, result).unwrap() {
                    assert!(sort.awaiting());
                    log.push((actual[a].1, actual[b].1));
                    result = Some(actual[a].0.cmp(&actual[b].0) as i32);
                }
                assert!(!sort.awaiting());
                assert_eq!(actual, expected, "len {len} descending {descending}");
                assert_eq!(log, expected_log, "len {len} descending {descending}");
                let keys: Vec<i32> = actual.iter().map(|item| item.0).collect();
                let mut sorted = keys.clone();
                sorted.sort_unstable();
                if descending {
                    sorted.reverse();
                }
                assert_eq!(keys, sorted);
            }
        }
    }
}
