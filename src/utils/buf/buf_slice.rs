use std::{ops::Range, sync::Arc};

use super::{BufWtr, OwnedBufWtr};

pub struct BufSlice {
    buf: Arc<OwnedBufWtr>,
    range: Range<usize>,
}

pub struct BufSliceBuilder {
    pub buf: Arc<OwnedBufWtr>,
    pub range: Range<usize>,
}

impl BufSliceBuilder {
    pub fn build(self) -> Result<BufSlice, Error> {
        if !(self.range.start <= self.range.end) {
            return Err(Error::IndexOutOfRange);
        }
        if !(self.range.end <= self.buf.data_len()) {
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
        assert!(self.range.end <= self.buf.data_len());
    }

    pub fn from_wtr(wtr: OwnedBufWtr) -> Self {
        let data_len = wtr.data_len();
        let this = BufSliceBuilder {
            buf: Arc::new(wtr),
            range: 0..data_len,
        }
        .build()
        .unwrap();
        this.check_rep();
        this
    }

    pub fn from_bytes(buf: Vec<u8>) -> Self {
        let buf_len = buf.len();
        let wtr = OwnedBufWtr::from_bytes(buf, 0, buf_len);
        let this = Self::from_wtr(wtr);
        this.check_rep();
        this
    }

    #[must_use]
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.buf.data()[self.range.start..self.range.end]
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

    use super::{BufSlice, BufSliceBuilder, BufWtr, OwnedBufWtr};

    #[test]
    fn data() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2]).unwrap();
        let slice = BufSliceBuilder {
            buf: Arc::new(buf),
            range: 1..3,
        }
        .build()
        .unwrap();
        assert_eq!(slice.data(), vec![1, 2]);
    }

    #[test]
    fn slice() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2, 3]).unwrap();
        let slice = BufSliceBuilder {
            buf: Arc::new(buf),
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
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![9, 0, 1, 2, 3]).unwrap();
        let slice = BufSliceBuilder {
            buf: Arc::new(buf),
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
}
