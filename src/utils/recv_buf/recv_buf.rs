use std::collections::VecDeque;

use crate::utils::Seq;

use super::{rwnd::Rwnd, SeqLocationToRwnd};

pub struct RecvBuf<TSeq, T>
where
    TSeq: Seq,
{
    rwnd: Rwnd<TSeq, T>,
    sorted: VecDeque<T>,
    len: usize,
}

impl<TSeq, T> RecvBuf<TSeq, T>
where
    TSeq: Seq,
{
    fn check_rep(&self) {
        let ofo_len = self.rwnd.size();
        assert_eq!(ofo_len + self.sorted.len(), self.len);
    }

    #[must_use]
    pub fn new(len: usize) -> Self {
        let this = RecvBuf {
            rwnd: Rwnd::new(len),
            sorted: VecDeque::new(),
            len,
        };
        this.check_rep();
        this
    }

    #[must_use]
    pub fn pop_front(&mut self) -> Option<T> {
        if let Some(x) = self.sorted.pop_front() {
            self.rwnd.increment_size();
            self.check_rep();
            Some(x)
        } else {
            self.check_rep();
            None
        }
    }

    #[must_use]
    pub fn insert(&mut self, seq: TSeq, v: T) -> SeqLocationToRwnd {
        let location = self.rwnd.location(seq);
        match location {
            SeqLocationToRwnd::InRecvWindow => {
                self.rwnd.insert(seq, v);
            }
            SeqLocationToRwnd::TooLate => (),
            SeqLocationToRwnd::TooEarly => (),
            SeqLocationToRwnd::AtRecvWindowStart => {
                // skip inserting this consecutive fragment to rwnd
                // hot path
                let v = self.rwnd.insert_then_pop_next(seq, v).unwrap();
                self.sorted.push_back(v);

                while let Some(v) = self.rwnd.pop_next() {
                    self.sorted.push_back(v);
                }
            }
        }
        self.check_rep();
        location
    }

    #[must_use]
    pub fn next_seq_to_receive(&self) -> TSeq {
        self.rwnd.start()
    }

    #[must_use]
    pub fn rwnd_size(&self) -> usize {
        self.rwnd.size()
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::{Seq32, SeqLocationToRwnd};

    use super::RecvBuf;

    #[test]
    fn test1() {
        let mut buf = RecvBuf::new(5);

        //         0  1  2  3  4
        // rwnd   [             ]
        // sorted I

        assert!(buf.pop_front().is_none());

        let location = buf.insert(Seq32::from_u32(5), 5);

        match location {
            SeqLocationToRwnd::TooEarly => (),
            _ => panic!(),
        }

        let location = buf.insert(Seq32::from_u32(1), 1);

        //         0  1  2  3  4
        // rwnd   [   1         ]
        // sorted I

        match location {
            SeqLocationToRwnd::InRecvWindow => (),
            _ => panic!(),
        }

        assert!(buf.pop_front().is_none());

        let location = buf.insert(Seq32::from_u32(0), 0);

        //         0  1  2  3  4
        // rwnd         [       ]
        // sorted [0  1]

        match location {
            SeqLocationToRwnd::AtRecvWindowStart => (),
            _ => panic!(),
        }

        assert_eq!(buf.pop_front().unwrap(), 0);

        //         0  1  2  3  4  5
        // rwnd         [          ]
        // sorted    [1]

        let location = buf.insert(Seq32::from_u32(1), 1);

        match location {
            SeqLocationToRwnd::TooLate => (),
            _ => panic!(),
        }

        let location = buf.insert(Seq32::from_u32(5), 5);

        //         0  1  2  3  4  5
        // rwnd         [         5]
        // sorted    [1]

        match location {
            SeqLocationToRwnd::InRecvWindow => (),
            _ => panic!(),
        }

        assert_eq!(buf.pop_front().unwrap(), 1);

        //         0  1  2  3  4  5  6
        // rwnd         [         5   ]
        // sorted      ][
    }
}
