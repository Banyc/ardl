use std::{ops::Range, sync::Arc};

use super::{BufWtr, OwnedBufWtr};

pub struct BufFrag {
    buf: Arc<OwnedBufWtr>,
    range: Range<usize>,
}

pub struct BufFragBuilder {
    pub buf: Arc<OwnedBufWtr>,
    pub range: Range<usize>,
}

impl BufFragBuilder {
    pub fn build(self) -> BufFrag {
        let this = BufFrag {
            buf: self.buf,
            range: self.range,
        };
        this.check_rep();
        this
    }
}

#[derive(Debug)]
pub enum Error {
    IndexOutOfRange,
}

impl BufFrag {
    #[inline]
    fn check_rep(&self) {
        assert!(self.range.start <= self.range.end);
        assert!(self.range.end <= self.buf.data_len());
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.buf.data()[self.range.start..self.range.end]
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.range.end - self.range.start
    }

    pub fn slice(&self, range: Range<usize>) -> Result<BufFrag, Error> {
        if !(range.start <= self.range.end) {
            return Err(Error::IndexOutOfRange);
        }
        if !(range.end <= self.buf.data_len()) {
            return Err(Error::IndexOutOfRange);
        }
        let start = self.range.start + range.start;
        let end = self.range.start + range.end;
        let slice = BufFragBuilder {
            buf: Arc::clone(&self.buf),
            range: start..end,
        }
        .build();
        Ok(slice)
    }
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use crate::utils::{BufWtr, OwnedBufWtr};

    use super::BufFragBuilder;

    #[test]
    fn data() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2]).unwrap();
        let frag = BufFragBuilder {
            buf: Arc::new(buf),
            range: 1..3,
        }
        .build();
        assert_eq!(frag.data(), vec![1, 2]);
    }

    #[test]
    fn slice() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2, 3]).unwrap();
        let frag = BufFragBuilder {
            buf: Arc::new(buf),
            range: 1..4,
        }
        .build();
        
        // frag: [1, 2, 3]
        
        let slice = frag.slice(1..2).unwrap();
        assert_eq!(slice.data(), vec![2]);
        let slice = frag.slice(0..2).unwrap();
        assert_eq!(slice.data(), vec![1, 2]);
        let slice = frag.slice(2..3).unwrap();
        assert_eq!(slice.data(), vec![3]);
        let slice = frag.slice(0..3).unwrap();
        assert_eq!(slice.data(), vec![1, 2, 3]);
    }
}
