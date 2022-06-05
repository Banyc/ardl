use super::SeqLocationToRwnd;
use crate::utils::Seq;
use std::collections::BTreeMap;

pub struct Rwnd<TSeq, T>
where
    TSeq: Seq,
{
    wnd: BTreeMap<TSeq, T>,
    size: usize, // inclusive
    start: TSeq,
}

impl<TSeq, T> Rwnd<TSeq, T>
where
    TSeq: Seq,
{
    fn check_rep(&self) {
        assert!(self.wnd.len() <= self.size);
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
            start: TSeq::zero(),
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
    pub fn start(&self) -> TSeq {
        self.start
    }

    #[must_use]
    #[inline]
    pub fn is_acceptable(&self, seq: TSeq) -> bool {
        match self.location(seq) {
            SeqLocationToRwnd::InRecvWindow => true,
            SeqLocationToRwnd::AtRecvWindowStart => true,
            SeqLocationToRwnd::TooLate => false,
            SeqLocationToRwnd::TooEarly => false,
        }
    }

    #[must_use]
    #[inline]
    pub fn location(&self, seq: TSeq) -> SeqLocationToRwnd {
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
    pub fn insert(&mut self, seq: TSeq, v: T) -> Option<T> {
        if !self.is_acceptable(seq) {
            panic!("Sequence {:?} is out of the window", seq);
        }
        let ret = self.wnd.insert(seq, v);
        self.check_rep();
        ret
    }

    /// Try to bypass insertion
    #[must_use]
    pub fn insert_then_pop_next(&mut self, seq: TSeq, v: T) -> Option<T> {
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
        self.start = self.start.add_usize(1);
        self.size -= 1;
        self.check_rep();
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::{Seq32, SeqLocationToRwnd};

    use super::Rwnd;

    #[test]
    fn test1() {
        let mut rwnd = Rwnd::new(4);
        rwnd.insert(Seq32::from_u32(2), 2);
        // _ _ 2 _
        assert_eq!(rwnd.size, 4);

        rwnd.insert(Seq32::from_u32(0), 0);
        // 0 _ 2 _
        assert_eq!(rwnd.size, 4);

        let zero = rwnd.pop_next().unwrap();
        assert_eq!(zero, 0);
        // _ 2 _
        assert_eq!(rwnd.size, 3);

        let one = rwnd.insert_then_pop_next(Seq32::from_u32(1), 1).unwrap();
        assert_eq!(one, 1);
        // 2 _
        assert_eq!(rwnd.size, 2);

        match rwnd.location(Seq32::from_u32(2)) {
            SeqLocationToRwnd::AtRecvWindowStart => (),
            _ => panic!(),
        }
        match rwnd.location(Seq32::from_u32(3)) {
            SeqLocationToRwnd::InRecvWindow => (),
            _ => panic!(),
        }

        let two = rwnd.pop_next().unwrap();
        assert_eq!(two, 2);
        // _
        assert_eq!(rwnd.size, 1);

        let three = rwnd.insert_then_pop_next(Seq32::from_u32(3), 3).unwrap();
        assert_eq!(three, 3);
        // <empty>
        assert_eq!(rwnd.size, 0);

        match rwnd.location(Seq32::from_u32(3)) {
            SeqLocationToRwnd::TooLate => (),
            _ => panic!(),
        }
        match rwnd.location(Seq32::from_u32(4)) {
            SeqLocationToRwnd::TooEarly => (),
            _ => panic!(),
        }
    }
}
