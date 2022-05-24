use std::{
    io::{self, Cursor},
    sync::Arc,
};

use super::{BufFrag, BufFragBuilder, BufWtr, OwnedBufWtr};

pub struct BufRdr {
    buf: Arc<OwnedBufWtr>,
    cursor: usize,
}

#[derive(Debug)]
pub enum Error {
    NotEnoughSpace,
}

impl BufRdr {
    fn check_rep(&self) {
        assert!(self.cursor <= self.buf.data_len());
    }

    pub fn from_wtr(wtr: OwnedBufWtr) -> Self {
        let this = BufRdr {
            buf: Arc::new(wtr),
            cursor: 0,
        };
        this.check_rep();
        this
    }

    pub fn from_bytes(buf: Vec<u8>) -> Self {
        let buf_len = buf.len();
        let wtr = OwnedBufWtr::from_bytes(buf, 0, buf_len);
        let this = BufRdr::from_wtr(wtr);
        this.check_rep();
        this
    }

    pub fn skip(&mut self, len: usize) -> Result<(), Error> {
        if !(self.cursor + len <= self.buf.data_len()) {
            return Err(Error::NotEnoughSpace);
        }
        self.cursor += len;
        self.check_rep();
        Ok(())
    }

    pub fn slice(&mut self, len: usize) -> Result<BufFrag, Error> {
        let end = self.cursor + len;
        if !(end <= self.buf.data_len()) {
            return Err(Error::NotEnoughSpace);
        }
        let frag = BufFragBuilder {
            buf: Arc::clone(&self.buf),
            range: self.cursor..end,
        }
        .build();
        self.cursor = end;
        self.check_rep();
        Ok(frag)
    }

    pub fn try_slice(&mut self, max_len: usize) -> Option<BufFrag> {
        let end = usize::min(self.cursor + max_len, self.buf.data_len());
        if end == self.cursor {
            return None;
        }
        let frag = BufFragBuilder {
            buf: Arc::clone(&self.buf),
            range: self.cursor..end,
        }
        .build();
        self.cursor = end;
        self.check_rep();
        Some(frag)
    }

    pub fn get_peek_cursor(&self) -> io::Cursor<&[u8]> {
        let buf = &self.buf.data()[self.cursor..];
        let cursor = Cursor::new(buf);
        cursor
    }

    pub fn is_empty(&self) -> bool {
        self.cursor == self.buf.data_len()
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::{BufWtr, OwnedBufWtr};

    use super::BufRdr;

    #[test]
    fn try_slice() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2, 3, 4, 5]).unwrap();
        let mut rdr = BufRdr::from_wtr(buf);
        let frag0 = rdr.try_slice(1).unwrap();
        assert_eq!(frag0.data(), vec![0]);
        let frag12 = rdr.try_slice(2).unwrap();
        assert_eq!(frag12.data(), vec![1, 2]);
        assert!(!rdr.is_empty());
        let frag345 = rdr.try_slice(99).unwrap();
        assert_eq!(frag345.data(), vec![3, 4, 5]);
        assert!(rdr.is_empty());
        let frag_none = rdr.try_slice(1);
        assert!(frag_none.is_none());
    }

    #[test]
    fn slice() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2, 3, 4, 5]).unwrap();
        let mut rdr = BufRdr::from_wtr(buf);
        let frag0 = rdr.slice(1).unwrap();
        assert_eq!(frag0.data(), vec![0]);
        let frag12 = rdr.slice(2).unwrap();
        assert_eq!(frag12.data(), vec![1, 2]);
        assert!(!rdr.is_empty());
        let frag_err = rdr.slice(99);
        assert!(frag_err.is_err());
        let frag345 = rdr.slice(3).unwrap();
        assert_eq!(frag345.data(), vec![3, 4, 5]);
        assert!(rdr.is_empty());
        let frag_err = rdr.slice(1);
        assert!(frag_err.is_err());
    }
}
