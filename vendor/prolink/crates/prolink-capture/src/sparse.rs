// SPDX-License-Identifier: GPL-3.0-only

//! Bytes that arrive at offsets, with the holes between them kept as holes.
//!
//! Two of this crate's jobs are the same job: an IP datagram arrives as
//! fragments carrying a byte offset, and a TCP flow arrives as segments
//! carrying a sequence number. Both can arrive out of order, both can arrive
//! twice, and in both a piece can simply never arrive. The one thing neither
//! may do is close the hole up, because the protocols above have no framing
//! that would let a reader notice: a dbserver message is delimited by nothing
//! but its own contents, so concatenating across a gap does not fail, it
//! decodes the following bytes as a field and every message after that is one
//! position out.
//!
//! So [`Sparse`] never concatenates. It holds a list of [`Run`]s — maximal
//! stretches that did arrive contiguously — and a caller that needs the whole
//! thing asks [`Sparse::contiguous`], which answers `None` when there is a
//! hole rather than answering with bytes that are not what was sent.
//!
//! # Overlaps
//!
//! A retransmission normally repeats bytes already held. The policy here is
//! **first writer wins**: bytes already in a run are kept and the arriving
//! copy is used only where nothing is held yet. That is what a receiving TCP
//! stack does, so it reconstructs what the peer above the wire actually saw.
//! It also makes a capture recorded on two interfaces of one bridge — which is
//! what most of this project's corpus is — collapse to a single copy for free.

/// A stretch of bytes that arrived contiguously, and where it starts.
#[derive(Clone, PartialEq, Eq)]
pub struct Run {
    offset: u64,
    data: Vec<u8>,
}

impl Run {
    /// Offset of the first byte, from the start of the reassembled whole.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// One past the last byte.
    pub fn end(&self) -> u64 {
        self.offset
            .saturating_add(self.data.len().try_into().unwrap_or(u64::MAX))
    }

    /// The bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Run({}..{}, {} bytes)",
            self.offset,
            self.end(),
            self.data.len()
        )
    }
}

/// A stretch that never arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gap {
    /// Offset of the first missing byte.
    pub offset: u64,
    /// How many bytes are missing.
    pub len: u64,
}

/// Bytes placed at offsets, with the holes between them preserved.
///
/// The runs are kept sorted by offset, never overlapping and never merely
/// touching — two runs in the list always have a real hole between them, which
/// is what makes `runs.len() == 1` a proof of completeness rather than a hint.
#[derive(Clone, Default)]
pub(crate) struct Sparse {
    runs: Vec<Run>,
}

impl Sparse {
    /// An empty buffer.
    pub(crate) fn new() -> Self {
        Self { runs: Vec::new() }
    }

    /// Place `data` at `offset`, keeping any bytes already held there.
    pub(crate) fn insert(&mut self, offset: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let len: u64 = data.len().try_into().unwrap_or(u64::MAX);
        let end = offset.saturating_add(len);

        // Runs are sorted and disjoint, so the ones that overlap or touch
        // `offset..=end` form one contiguous window of the list.
        let first = self.runs.partition_point(|run| run.end() < offset);
        let last = self.runs.partition_point(|run| run.offset <= end);
        let Some(window) = self.runs.get(first..last) else {
            return;
        };

        let mut merged_start = offset;
        let mut merged_end = end;
        for run in window {
            merged_start = merged_start.min(run.offset);
            merged_end = merged_end.max(run.end());
        }
        let Ok(size) = usize::try_from(merged_end.saturating_sub(merged_start)) else {
            // A single run larger than this platform's address space cannot be
            // built, so there is nothing honest to do but leave it out.
            return;
        };

        let mut merged = vec![0u8; size];
        // The arriving bytes go down first and the bytes already held go on
        // top of them, which is what makes the policy first-writer-wins.
        copy_into(&mut merged, offset.saturating_sub(merged_start), data);
        for run in window {
            copy_into(
                &mut merged,
                run.offset.saturating_sub(merged_start),
                &run.data,
            );
        }
        self.runs.splice(
            first..last,
            [Run {
                offset: merged_start,
                data: merged,
            }],
        );
    }

    /// Shift every run forward by `amount`, to make room below offset zero.
    ///
    /// Needed when the first bytes seen on a TCP flow turn out not to be its
    /// earliest: the base has to move, and everything already held moves with
    /// it. See [`crate::tcp`].
    pub(crate) fn shift(&mut self, amount: u64) {
        for run in &mut self.runs {
            run.offset = run.offset.saturating_add(amount);
        }
    }

    /// The runs, in offset order.
    pub(crate) fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Everything, as one slice — or `None` when a hole splits it.
    ///
    /// Also `None` when the buffer starts above offset zero, because a missing
    /// prefix is a hole like any other.
    pub(crate) fn contiguous(&self) -> Option<&[u8]> {
        match self.runs.as_slice() {
            [only] if only.offset == 0 => Some(&only.data),
            _ => None,
        }
    }

    /// The holes, in offset order, including a leading one.
    pub(crate) fn gaps(&self) -> Vec<Gap> {
        let mut gaps = Vec::new();
        let mut cursor = 0u64;
        for run in &self.runs {
            if run.offset > cursor {
                gaps.push(Gap {
                    offset: cursor,
                    len: run.offset.saturating_sub(cursor),
                });
            }
            cursor = run.end();
        }
        gaps
    }

    /// How many bytes are held, not counting the holes.
    pub(crate) fn len(&self) -> u64 {
        self.runs
            .iter()
            .map(|run| run.end().saturating_sub(run.offset))
            .sum()
    }

    /// True when nothing has been placed yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

impl std::fmt::Debug for Sparse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(&self.runs).finish()
    }
}

/// Write `source` into `target` at `at`, dropping anything past the end.
fn copy_into(target: &mut [u8], at: u64, source: &[u8]) {
    let Ok(start) = usize::try_from(at) else {
        return;
    };
    let Some(end) = start.checked_add(source.len()) else {
        return;
    };
    if let Some(slot) = target.get_mut(start..end) {
        slot.copy_from_slice(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(sparse: &Sparse) -> Vec<(u64, Vec<u8>)> {
        sparse
            .runs()
            .iter()
            .map(|run| (run.offset(), run.data().to_vec()))
            .collect()
    }

    #[test]
    fn pieces_arriving_in_order_become_one_run() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abc");
        sparse.insert(3, b"def");
        assert_eq!(sparse.contiguous(), Some(b"abcdef".as_slice()));
        assert!(sparse.gaps().is_empty());
    }

    #[test]
    fn pieces_arriving_backwards_still_become_one_run() {
        let mut sparse = Sparse::new();
        sparse.insert(6, b"ghi");
        sparse.insert(0, b"abc");
        sparse.insert(3, b"def");
        assert_eq!(sparse.contiguous(), Some(b"abcdefghi".as_slice()));
    }

    #[test]
    fn a_hole_is_reported_rather_than_closed_up() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abc");
        sparse.insert(10, b"xyz");
        assert_eq!(
            sparse.contiguous(),
            None,
            "concatenating across a hole would be a lie"
        );
        assert_eq!(
            runs(&sparse),
            vec![(0, b"abc".to_vec()), (10, b"xyz".to_vec())]
        );
        assert_eq!(sparse.gaps(), vec![Gap { offset: 3, len: 7 }]);
        assert_eq!(sparse.len(), 6);
    }

    #[test]
    fn a_missing_prefix_is_a_hole_too() {
        let mut sparse = Sparse::new();
        sparse.insert(4, b"abc");
        assert_eq!(sparse.contiguous(), None);
        assert_eq!(sparse.gaps(), vec![Gap { offset: 0, len: 4 }]);
    }

    #[test]
    fn a_hole_closes_when_the_missing_piece_arrives() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abc");
        sparse.insert(6, b"ghi");
        sparse.insert(3, b"def");
        assert_eq!(sparse.contiguous(), Some(b"abcdefghi".as_slice()));
        assert_eq!(sparse.runs().len(), 1);
    }

    #[test]
    fn a_retransmission_does_not_disturb_bytes_already_held() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abcdef");
        // The same segment again, but corrupt. A receiver would have kept the
        // first copy, so this must too.
        sparse.insert(2, b"ZZ");
        assert_eq!(sparse.contiguous(), Some(b"abcdef".as_slice()));
    }

    #[test]
    fn an_overlapping_segment_contributes_only_its_new_bytes() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abc");
        sparse.insert(2, b"ZZZZ"); // covers 2..6; only 3..6 is new
        assert_eq!(sparse.contiguous(), Some(b"abcZZZ".as_slice()));
    }

    #[test]
    fn a_segment_spanning_several_runs_fills_only_the_holes() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"ab");
        sparse.insert(6, b"gh");
        sparse.insert(0, b"ZZZZZZZZ");
        assert_eq!(sparse.contiguous(), Some(b"abZZZZgh".as_slice()));
    }

    #[test]
    fn a_segment_between_two_runs_does_not_merge_them_when_it_reaches_neither() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"ab");
        sparse.insert(10, b"kl");
        sparse.insert(5, b"f");
        assert_eq!(sparse.runs().len(), 3);
        assert_eq!(
            sparse.gaps(),
            vec![Gap { offset: 2, len: 3 }, Gap { offset: 6, len: 4 }]
        );
    }

    #[test]
    fn duplicate_inserts_cost_nothing() {
        let mut sparse = Sparse::new();
        for _ in 0..5 {
            sparse.insert(0, b"abc");
        }
        assert_eq!(sparse.runs().len(), 1);
        assert_eq!(sparse.len(), 3);
    }

    #[test]
    fn shifting_moves_every_run() {
        let mut sparse = Sparse::new();
        sparse.insert(0, b"abc");
        sparse.insert(10, b"xyz");
        sparse.shift(4);
        assert_eq!(
            runs(&sparse),
            vec![(4, b"abc".to_vec()), (14, b"xyz".to_vec())]
        );
    }

    #[test]
    fn an_empty_insert_is_not_a_run() {
        let mut sparse = Sparse::new();
        sparse.insert(7, b"");
        assert!(sparse.is_empty());
        assert_eq!(sparse.contiguous(), None);
    }
}
