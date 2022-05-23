use std::{rc::Rc, io::{self, Cursor}};

use super::{BufFrag, BufFragBuilder, OwnedBufWtr, BufWtr};

pub struct BufRdr {
    buf: Rc<OwnedBufWtr>,
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

    pub fn new(buf: OwnedBufWtr) -> Self {
        let this = BufRdr {
            buf: Rc::new(buf),
            cursor: 0,
        };
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

    pub fn try_slice(&mut self, len: usize) -> Option<BufFrag> {
        let end = usize::min(self.cursor + len, self.buf.data_len());
        if end == self.cursor {
            return None;
        }
        let frag = BufFragBuilder {
            buf: Rc::clone(&self.buf),
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
    use crate::utils::{OwnedBufWtr, BufWtr};

    use super::BufRdr;

    #[test]
    fn try_read() {
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2, 3, 4, 5]).unwrap();
        let mut rdr = BufRdr::new(buf);
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
}
