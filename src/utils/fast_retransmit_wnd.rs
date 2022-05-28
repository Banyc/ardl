use std::ops::Range;

use crate::utils::{dup::DuplicateThreshold, Seq};

pub struct FastRetransmissionWnd {
    start: Seq,
    end: Seq, // exclusive
    duplicate_threshold: DuplicateThreshold<Seq>,
}

impl FastRetransmissionWnd {
    fn check_rep(&self) {
        assert!(self.start <= self.end);
    }

    pub fn new(nack_duplicate_limit_to_activate: usize) -> Self {
        let this = FastRetransmissionWnd {
            start: Seq::from_u32(0),
            end: Seq::from_u32(0),
            duplicate_threshold: DuplicateThreshold::new(
                Seq::from_u32(0),
                nack_duplicate_limit_to_activate,
            ),
        };
        this.check_rep();
        this
    }

    pub fn contains(&self, seq: Seq) -> bool {
        self.duplicate_threshold.is_activated() && self.start <= seq && seq < self.end
    }

    pub fn retransmitted(&mut self, seq: Seq) {
        assert!(self.contains(seq));
        self.start = seq.add_u32(1);
        self.check_rep();
    }

    pub fn try_set_boundaries(&mut self, range: Range<Seq>) {
        assert!(range.start <= range.end);
        self.duplicate_threshold.set(range.start);
        if self.duplicate_threshold.is_activated() {
            self.start = range.start;
            self.end = range.end;
        }
        self.check_rep();
    }
}
