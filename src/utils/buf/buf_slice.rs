use std::{ops::Range, sync::Arc};

pub struct BufSlice {
    buf: Arc<Vec<u8>>,
    range: Range<usize>,
}

pub struct BufSliceBuilder {
    pub buf: Arc<Vec<u8>>,
    pub range: Range<usize>,
}

impl BufSliceBuilder {
    pub fn build(self) -> Result<BufSlice, Error> {
        if !(self.range.start <= self.range.end) {
            return Err(Error::IndexOutOfRange);
        }
        if !(self.range.end <= self.buf.len()) {
            return Err(Error::IndexOutOfRange);
        }

        let this = BufSlice {
            buf: self.buf,
            range: self.range,
        };
        this.check_rep();
        Ok(this)
    }
}

impl BufSlice {
    #[inline]
    fn check_rep(&self) {
        assert!(self.range.start <= self.range.end);
        assert!(self.range.end <= self.buf.len());
    }

    pub fn clone(slice: &Self) -> Self {
        let clone = slice.slice(0..slice.len()).unwrap();
        clone
    }

    pub fn from_bytes(buf: Vec<u8>) -> Self {
        let buf_len = buf.len();
        let this = Self {
            buf: Arc::new(buf),
            range: 0..buf_len,
        };
        this.check_rep();
        this
    }

    #[must_use]
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.buf[self.range.start..self.range.end]
    }

    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.range.end - self.range.start
    }

    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Result<BufSlice, Error> {
        let start = self.range.start + range.start;
        let end = self.range.start + range.end;
        let slice = BufSliceBuilder {
            buf: Arc::clone(&self.buf),
            range: start..end,
        }
        .build()?;
        Ok(slice)
    }

    #[must_use]
    pub fn split(&self, mid: usize) -> Result<(BufSlice, BufSlice), Error> {
        let range_mid = self.range.start + mid;
        let head = BufSliceBuilder {
            buf: Arc::clone(&self.buf),
            range: self.range.start..range_mid,
        }
        .build()?;
        let tail = BufSliceBuilder {
            buf: Arc::clone(&self.buf),
            range: range_mid..self.range.end,
        }
        .build()?;
        Ok((head, tail))
    }

    #[must_use]
    #[inline]
    pub fn pop_front(&mut self, len: usize) -> Result<BufSlice, Error> {
        let range_mid = self.range.start + len;
        let front = BufSliceBuilder {
            buf: Arc::clone(&self.buf),
            range: self.range.start..range_mid,
        }
        .build()?;
        self.range.start = range_mid;
        Ok(front)
    }
}

#[derive(Debug)]
pub enum Error {
    IndexOutOfRange,
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use super::{BufSlice, BufSliceBuilder};

    #[test]
    fn data() {
        let slice = BufSliceBuilder {
            buf: Arc::new(vec![0, 1, 2]),
            range: 1..3,
        }
        .build()
        .unwrap();
        assert_eq!(slice.data(), vec![1, 2]);
    }

    #[test]
    fn slice() {
        let slice = BufSliceBuilder {
            buf: Arc::new(vec![0, 1, 2, 3]),
            range: 1..4,
        }
        .build()
        .unwrap();

        // slice: [1, 2, 3]

        let sub_slice = slice.slice(1..2).unwrap();
        assert_eq!(sub_slice.data(), vec![2]);
        let sub_slice = slice.slice(0..2).unwrap();
        assert_eq!(sub_slice.data(), vec![1, 2]);
        let sub_slice = slice.slice(2..3).unwrap();
        assert_eq!(sub_slice.data(), vec![3]);
        let sub_slice = slice.slice(0..3).unwrap();
        assert_eq!(sub_slice.data(), vec![1, 2, 3]);
    }

    #[test]
    fn split() {
        let slice = BufSliceBuilder {
            buf: Arc::new(vec![9, 0, 1, 2, 3]),
            range: 1..4,
        }
        .build()
        .unwrap();

        // slice: [0, 1, 2]

        let (head, tail) = slice.split(1).unwrap();
        assert_eq!(head.data(), vec![0]);
        assert_eq!(tail.data(), vec![1, 2]);
    }

    #[test]
    fn pop_front() {
        let mut buf = BufSlice::from_bytes(vec![0, 1, 2, 3, 4, 5]);
        let slice0 = buf.pop_front(1).unwrap();
        assert_eq!(slice0.data(), vec![0]);
        let slice12 = buf.pop_front(2).unwrap();
        assert_eq!(slice12.data(), vec![1, 2]);
        assert!(!buf.is_empty());
        let slice_err = buf.pop_front(99);
        assert!(slice_err.is_err());
        let slice345 = buf.pop_front(3).unwrap();
        assert_eq!(slice345.data(), vec![3, 4, 5]);
        assert!(buf.is_empty());
        let slice_err = buf.pop_front(1);
        assert!(slice_err.is_err());
    }

    #[test]
    fn clone() {
        let slice1 = BufSlice::from_bytes(vec![0, 1, 2, 3, 4, 5]);
        let slice2 = BufSlice::clone(&slice1);
        assert_eq!(slice1.data(), slice2.data());
    }
}
