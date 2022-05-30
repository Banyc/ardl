use std::collections::{btree_map, BTreeMap};

use crate::utils::Seq;

pub struct Swnd<TSeq, T>
where
    TSeq: Seq,
{
    wnd: BTreeMap<TSeq, T>,
    remote_rwnd_size: usize,
    end: TSeq, // exclusive
    wnd_size_cap: usize,
}

impl<TSeq, T> Swnd<TSeq, T>
where
    TSeq: Seq,
{
    fn check_rep(&self) {
        assert!(self.wnd.len() <= self.wnd_size_cap);
        assert!(self.start() <= self.end);
        // TODO: move this invariant outside
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
            end: TSeq::zero(),
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
    pub fn end(&self) -> TSeq {
        self.end
    }

    #[must_use]
    pub fn iter_mut(&mut self) -> btree_map::IterMut<'_, TSeq, T> {
        self.wnd.iter_mut()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        let size = self.size();
        usize::max(self.remote_rwnd_size, 1) <= size || self.wnd_size_cap <= size
    }

    #[must_use]
    fn start(&self) -> TSeq {
        let mut first = None;
        for (&seq, _) in &self.wnd {
            first = Some(seq);
            break;
        }
        match first {
            Some(x) => x,
            None => self.end,
        }
    }

    /// Unit: sequence
    #[must_use]
    pub fn size(&self) -> usize {
        self.end.sub(&self.start()) as usize
    }

    pub fn push_back(&mut self, v: T) {
        assert!(!self.is_full());
        // println!("swnd: push_back: start: {:?}", self.start());
        // println!("swnd: push_back: end: {:?}", self.end);
        self.wnd.insert(self.end, v);
        self.end = self.end.add_usize(1);
        self.check_rep();
    }

    pub fn remove(&mut self, ack: &TSeq) -> Option<T> {
        // println!("swnd: remove: {:?}", ack);
        let ret = self.wnd.remove(ack);
        self.check_rep();
        ret
    }

    pub fn remove_before(&mut self, nack: TSeq) {
        let mut to_removes = Vec::new();
        for (&seq, _) in &self.wnd {
            if seq < nack {
                to_removes.push(seq);
            } else {
                break;
            }
        }
        for to_remove in to_removes {
            // println!("swnd: remove_before: {:?}", to_remove);
            self.wnd.remove(&to_remove);
        }
        self.check_rep();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::utils::Seq32;

    use super::Swnd;

    #[test]
    fn btree_map_iter() {
        let mut map = BTreeMap::new();
        map.insert(Seq32::from_u32(2), 2);
        map.insert(Seq32::from_u32(1), 1);
        map.insert(Seq32::from_u32(3), 3);
        map.insert(Seq32::from_u32(0), 0);

        let mut i = 0;
        for (&k, &v) in &map {
            assert_eq!(k.to_u32(), i);
            assert_eq!(v, i);
            i += 1;
        }

        let mut i = 0;
        for (&k, &v) in map.iter() {
            assert_eq!(k.to_u32(), i);
            assert_eq!(v, i);
            i += 1;
        }

        let mut i = 0;
        for (&k, &mut v) in map.iter_mut() {
            assert_eq!(k.to_u32(), i);
            assert_eq!(v, i);
            i += 1;
        }
    }

    #[test]
    fn test1() {
        let mut wnd = Swnd::<Seq32, i32>::new(3);
        assert_eq!(wnd.start().to_u32(), 0);
        assert_eq!(wnd.size(), 0);
        assert_eq!(wnd.end().to_u32(), 0);

        wnd.push_back(0);

        // max(rwnd, 1): [ ]
        // cap:          [       ]
        //                0  1  2  3  4  5  6
        //               [0]

        assert_eq!(wnd.start().to_u32(), 0);
        assert_eq!(wnd.size(), 1);
        assert_eq!(wnd.end().to_u32(), 1);

        assert!(wnd.is_full());

        wnd.set_remote_rwnd_size(1);

        assert!(wnd.is_full());

        wnd.set_remote_rwnd_size(2);

        // max(rwnd, 1): [    ]
        // cap:          [       ]
        //                0  1  2  3  4  5  6
        //               [0]

        assert!(!wnd.is_full());

        assert_eq!(wnd.start().to_u32(), 0);
        assert_eq!(wnd.size(), 1);
        assert_eq!(wnd.end().to_u32(), 1);

        wnd.remove(&Seq32::from_u32(0));

        // max(rwnd, 1):    [    ]
        // cap:             [       ]
        //                0  1  2  3  4  5  6
        //                 ][

        assert_eq!(wnd.start().to_u32(), 1);
        assert_eq!(wnd.size(), 0);
        assert_eq!(wnd.end().to_u32(), 1);

        wnd.push_back(1);

        // max(rwnd, 1):    [    ]
        // cap:             [       ]
        //                0  1  2  3  4  5  6
        //                  [1]

        assert_eq!(wnd.start().to_u32(), 1);
        assert_eq!(wnd.size(), 1);
        assert_eq!(wnd.end().to_u32(), 2);

        wnd.push_back(2);

        // max(rwnd, 1):    [    ]
        // cap:             [       ]
        //                0  1  2  3  4  5  6
        //                  [1  2]

        assert_eq!(wnd.start().to_u32(), 1);
        assert_eq!(wnd.size(), 2);
        assert_eq!(wnd.end().to_u32(), 3);

        assert!(wnd.is_full());
    }
}
