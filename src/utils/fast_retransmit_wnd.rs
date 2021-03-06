use crate::utils::{dup::DuplicateThreshold, Seq};
use std::ops::Range;

pub struct FastRetransmissionWnd<TSeq>
where
    TSeq: Seq,
{
    start: TSeq,
    end: TSeq, // exclusive
    duplicate_threshold: DuplicateThreshold<TSeq>,
}

impl<TSeq> FastRetransmissionWnd<TSeq>
where
    TSeq: Seq,
{
    fn check_rep(&self) {
        assert!(self.start <= self.end);
    }

    pub fn new(nack_duplicate_limit_to_activate: usize) -> Self {
        let this = FastRetransmissionWnd {
            start: Seq::zero(),
            end: Seq::zero(),
            duplicate_threshold: DuplicateThreshold::new(
                Seq::zero(),
                nack_duplicate_limit_to_activate,
            ),
        };
        this.check_rep();
        this
    }

    pub fn contains(&self, seq: TSeq) -> bool {
        self.start <= seq && seq < self.end
    }

    pub fn start(&self) -> TSeq {
        self.start
    }

    pub fn end(&self) -> TSeq {
        self.end
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    pub fn retransmitted(&mut self, seq: TSeq) {
        assert!(self.contains(seq));
        self.start = seq.add_usize(1);
        self.check_rep();
    }

    pub fn try_set_boundaries(&mut self, range: Range<TSeq>) {
        assert!(range.start <= range.end);
        self.duplicate_threshold.set(range.start);
        if self.duplicate_threshold.is_activated() {
            self.start = range.start;
            self.end = range.end;
            self.duplicate_threshold.recount();
        }
        self.check_rep();
    }
}
