use std::collections::VecDeque;

use super::{BufFrag, BufRdr};

pub struct BufSlicer {
    queue: VecDeque<BufRdr>,
    byte_len: usize,
    byte_cap: usize,
}

pub struct PushError<T>(pub T);

impl BufSlicer {
    fn check_rep(&self) {
        assert!(self.byte_len <= self.byte_cap);
        if !self.queue.is_empty() {
            assert!(!self.queue.front().unwrap().is_empty());
        }
    }

    pub fn new(byte_cap: usize) -> Self {
        let this = BufSlicer {
            queue: VecDeque::new(),
            byte_len: 0,
            byte_cap,
        };
        this.check_rep();
        this
    }

    pub fn push_back(&mut self, rdr: BufRdr) -> Result<(), PushError<BufRdr>> {
        let rdr_len = rdr.len();
        if !(self.byte_len + rdr_len <= self.byte_cap) {
            return Err(PushError(rdr));
        }
        if rdr.is_empty() {
            return Ok(());
        }

        self.byte_len += rdr_len;
        self.queue.push_back(rdr);
        self.check_rep();
        Ok(())
    }

    pub fn slice_front(&mut self, max_len: usize) -> BufFrag {
        let mut rdr = self.queue.pop_front().unwrap();
        let buf = rdr.try_slice(max_len).unwrap();
        self.byte_len -= buf.len();
        if !rdr.is_empty() {
            self.queue.push_front(rdr);
        }
        self.check_rep();
        buf
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::BufRdr;

    use super::BufSlicer;

    #[test]
    fn test1() {
        let mut slicer = BufSlicer::new(3);

        let rdr1 = BufRdr::from_bytes(vec![0]);
        let rdr2 = BufRdr::from_bytes(vec![1, 2]);
        let rdr3 = BufRdr::from_bytes(vec![3]);

        slicer.push_back(rdr1).map_err(|_| ()).unwrap();
        slicer.push_back(rdr2).map_err(|_| ()).unwrap();
        assert!(slicer.push_back(rdr3).is_err());

        let slice1 = slicer.slice_front(2);
        assert_eq!(slice1.data(), vec![0]);

        let slice2 = slicer.slice_front(1);
        assert_eq!(slice2.data(), vec![1]);

        let slice3 = slicer.slice_front(2);
        assert_eq!(slice3.data(), vec![2]);

        assert!(slicer.is_empty());
    }
}
