use std::collections::BTreeMap;

use crate::utils::{Seq, SlidingWndKey};

use super::SeqLocationToRwnd;

pub struct Rwnd<T> {
    wnd: BTreeMap<Seq, T>,
    size: usize, // inclusive
    start: Seq,
}

impl<T> Rwnd<T> {
    fn check_rep(&self) {
        assert!(self.wnd.len() <= self.size);
        assert!(self.size <= u32::MAX as usize);
        // for (&seq, _) in &self.wnd {
        //     assert!(self.next_seq_to_receive < seq);
        //     break;
        // }
    }

    #[must_use]
    pub fn new(size: usize) -> Self {
        let this = Rwnd {
            wnd: BTreeMap::new(),
            size,
            start: Seq::from_u32(0),
        };
        this.check_rep();
        this
    }

    #[inline]
    pub fn increment_size(&mut self) {
        self.size += 1;
        self.check_rep();
    }

    #[must_use]
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    #[inline]
    pub fn start(&self) -> Seq {
        self.start
    }

    #[must_use]
    #[inline]
    pub fn is_acceptable(&self, seq: Seq) -> bool {
        match self.location(seq) {
            SeqLocationToRwnd::InRecvWindow => true,
            SeqLocationToRwnd::AtRecvWindowStart => true,
            SeqLocationToRwnd::TooLate => false,
            SeqLocationToRwnd::TooEarly => false,
        }
    }

    #[must_use]
    #[inline]
    pub fn location(&self, seq: Seq) -> SeqLocationToRwnd {
        if !(self.start <= seq) {
            SeqLocationToRwnd::TooLate
        } else if !(seq < self.start.add_usize(self.size)) {
            SeqLocationToRwnd::TooEarly
        } else if self.start == seq {
            SeqLocationToRwnd::AtRecvWindowStart
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
        if !self.is_acceptable(seq) {
            panic!("Sequence {:?} is out of the window", seq);
        }
        if seq == self.start {
            self.wnd_proceed();
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
        if let Some(v) = self.wnd.remove(&self.start) {
            self.wnd_proceed();
            self.check_rep();
            Some(v)
        } else {
            self.check_rep();
            None
        }
    }

    #[inline]
    fn wnd_proceed(&mut self) {
        self.start.increment();
        self.size -= 1;
        self.check_rep();
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::{Seq, SeqLocationToRwnd};

    use super::Rwnd;

    #[test]
    fn test1() {
        let mut rwnd = Rwnd::new(4);
        rwnd.insert(Seq::from_u32(2), 2);
        // _ _ 2 _
        assert_eq!(rwnd.size, 4);

        rwnd.insert(Seq::from_u32(0), 0);
        // 0 _ 2 _
        assert_eq!(rwnd.size, 4);

        let zero = rwnd.pop_next().unwrap();
        assert_eq!(zero, 0);
        // _ 2 _
        assert_eq!(rwnd.size, 3);

        let one = rwnd.insert_then_pop_next(Seq::from_u32(1), 1).unwrap();
        assert_eq!(one, 1);
        // 2 _
        assert_eq!(rwnd.size, 2);

        match rwnd.location(Seq::from_u32(2)) {
            SeqLocationToRwnd::AtRecvWindowStart => (),
            _ => panic!(),
        }
        match rwnd.location(Seq::from_u32(3)) {
            SeqLocationToRwnd::InRecvWindow => (),
            _ => panic!(),
        }

        let two = rwnd.pop_next().unwrap();
        assert_eq!(two, 2);
        // _
        assert_eq!(rwnd.size, 1);

        let three = rwnd.insert_then_pop_next(Seq::from_u32(3), 3).unwrap();
        assert_eq!(three, 3);
        // <empty>
        assert_eq!(rwnd.size, 0);

        match rwnd.location(Seq::from_u32(3)) {
            SeqLocationToRwnd::TooLate => (),
            _ => panic!(),
        }
        match rwnd.location(Seq::from_u32(4)) {
            SeqLocationToRwnd::TooEarly => (),
            _ => panic!(),
        }
    }
}
