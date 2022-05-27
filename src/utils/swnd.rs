use std::collections::{btree_map, BTreeMap};

use crate::utils::Seq;

pub struct Swnd<T> {
    wnd: BTreeMap<Seq, T>,
    remote_rwnd_size: usize,
    end: Seq, // exclusive
    wnd_size_cap: usize,
}

impl<T> Swnd<T> {
    fn check_rep(&self) {
        assert!(self.wnd.len() <= self.wnd_size_cap);
        assert!(self.remote_rwnd_size <= u32::MAX as usize);
        for (&seq, _) in &self.wnd {
            assert!(seq < self.end);
        }
    }

    #[must_use]
    pub fn new(wnd_size_cap: usize) -> Self {
        let this = Swnd {
            wnd: BTreeMap::new(),
            remote_rwnd_size: 0,
            end: Seq::from_u32(0),
            wnd_size_cap,
        };
        this.check_rep();
        this
    }

    pub fn set_remote_rwnd_size(&mut self, n: usize) {
        self.remote_rwnd_size = n;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.wnd.len() == 0
    }

    #[must_use]
    pub fn end(&self) -> Seq {
        self.end
    }

    #[must_use]
    pub fn iter_mut(&mut self) -> btree_map::IterMut<'_, Seq, T> {
        self.wnd.iter_mut()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        let size = self.size();
        usize::max(self.remote_rwnd_size, 1) <= size || self.wnd_size_cap <= size
    }

    #[must_use]
    fn start(&self) -> Seq {
        let mut first = None;
        for (&seq, _) in &self.wnd {
            first = Some(seq);
        }
        match first {
            Some(x) => x,
            None => self.end,
        }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.end.sub_seq(self.start()) as usize
    }

    pub fn push_back(&mut self, v: T) {
        assert!(!self.is_full());
        self.wnd.insert(self.end, v);
        self.end.increment();
        self.check_rep();
    }

    pub fn remove(&mut self, ack: &Seq) -> Option<T> {
        let ret = self.wnd.remove(ack);
        self.check_rep();
        ret
    }

    pub fn remove_before(&mut self, nack: Seq) {
        let mut to_removes = Vec::new();
        for (&seq, _) in &self.wnd {
            if seq < nack {
                to_removes.push(seq);
            } else {
                break;
            }
        }
        for to_remove in to_removes {
            self.wnd.remove(&to_remove);
        }
        self.check_rep();
    }
}
