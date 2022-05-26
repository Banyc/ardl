use std::collections::BTreeMap;

use crate::utils::Seq;

pub struct Rwnd<T> {
    wnd: BTreeMap<Seq, T>,
    max_len: usize, // inclusive
    next_seq_to_receive: Seq,
}

pub enum SeqLocationToRwnd {
    InRecvWindow,
    TooLate,
    TooEarly,
}

impl<T> Rwnd<T> {
    fn check_rep(&self) {
        assert!(self.max_len > 0);
        assert!(self.wnd.len() <= self.max_len);
        assert!(self.max_len <= u32::MAX as usize);
    }

    #[must_use]
    pub fn new(max_len: usize) -> Self {
        let this = Rwnd {
            wnd: BTreeMap::new(),
            max_len,
            next_seq_to_receive: Seq::from_u32(0),
        };
        this.check_rep();
        this
    }

    #[must_use]
    #[inline]
    pub fn max_len(&self) -> usize {
        self.max_len
    }

    #[must_use]
    #[inline]
    pub fn next_seq_to_receive(&self) -> Seq {
        self.next_seq_to_receive
    }

    #[must_use]
    #[inline]
    pub fn wnd(&self) -> &BTreeMap<Seq, T> {
        &self.wnd
    }

    #[must_use]
    #[inline]
    pub fn free_len(&self) -> usize {
        self.max_len - self.wnd.len()
    }

    #[must_use]
    #[inline]
    pub fn is_acceptable(&self, seq: Seq) -> bool {
        if let SeqLocationToRwnd::InRecvWindow = self.location(seq) {
            true
        } else {
            false
        }
    }

    #[must_use]
    #[inline]
    pub fn location(&self, seq: Seq) -> SeqLocationToRwnd {
        if !(self.next_seq_to_receive <= seq) {
            SeqLocationToRwnd::TooLate
        } else if !(seq < self.next_seq_to_receive.add_u32(self.max_len as u32)) {
            SeqLocationToRwnd::TooEarly
        } else {
            SeqLocationToRwnd::InRecvWindow
        }
    }

    #[inline]
    pub fn insert(&mut self, seq: Seq, v: T) -> Option<T> {
        if !self.is_acceptable(seq) {
            panic!("Sequence {:?} is out of the window", seq);
        }
        let ret = self.wnd.insert(seq, v);
        self.check_rep();
        ret
    }

    /// Try to bypass insertion
    #[must_use]
    pub fn insert_then_pop_next(&mut self, seq: Seq, v: T) -> Option<T> {
        if seq == self.next_seq_to_receive {
            self.next_seq_to_receive.increment();
            self.check_rep();
            Some(v)
        } else {
            self.insert(seq, v);
            self.check_rep();
            None
        }
    }

    #[must_use]
    #[inline]
    pub fn pop_next(&mut self) -> Option<T> {
        if let Some(v) = self.wnd.remove(&self.next_seq_to_receive) {
            self.next_seq_to_receive.increment();
            self.check_rep();
            Some(v)
        } else {
            self.check_rep();
            None
        }
    }
}
