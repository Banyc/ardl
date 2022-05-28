use super::{BufSlice, OwnedBufWtr};

pub struct BufRdr {
    buf: BufSlice,
}

#[derive(Debug)]
pub enum Error {
    NotEnoughSpace,
}

impl BufRdr {
    fn check_rep(&self) {}

    #[must_use]
    pub fn from_slice(slice: BufSlice) -> Self {
        let this = BufRdr { buf: slice };
        this.check_rep();
        this
    }

    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let this = BufRdr::from_slice(BufSlice::from_bytes(bytes));
        this.check_rep();
        this
    }

    #[must_use]
    pub fn from_wtr(wtr: OwnedBufWtr) -> Self {
        let this = BufRdr::from_slice(BufSlice::from_wtr(wtr));
        this.check_rep();
        this
    }

    pub fn skip_precisely(&mut self, len: usize) -> Result<(), Error> {
        let _ = self.take_precisely(len)?;
        Ok(())
    }

    #[must_use]
    pub fn take_precisely(&mut self, len: usize) -> Result<BufSlice, Error> {
        let (head, tail) = self.buf.split(len).map_err(|_| Error::NotEnoughSpace)?;
        self.buf = tail;
        self.check_rep();
        Ok(head)
    }

    #[must_use]
    pub fn take_max(&mut self, max_len: usize) -> BufSlice {
        let len = usize::min(self.buf.len(), max_len);

        let slice = self.take_precisely(len).unwrap();
        self.check_rep();
        slice
    }

    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.buf.data()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::buf::BufSlice;

    use super::BufRdr;

    #[test]
    fn take_max() {
        let buf = BufSlice::from_bytes(vec![0, 1, 2, 3, 4, 5]);
        let mut rdr = BufRdr::from_slice(buf);
        let frag0 = rdr.take_max(1);
        assert_eq!(frag0.data(), vec![0]);
        let frag12 = rdr.take_max(2);
        assert_eq!(frag12.data(), vec![1, 2]);
        assert!(!rdr.is_empty());
        let frag345 = rdr.take_max(99);
        assert_eq!(frag345.data(), vec![3, 4, 5]);
        assert!(rdr.is_empty());
        let frag_none = rdr.take_max(1);
        assert!(frag_none.is_empty());
    }

    #[test]
    fn take_precisely() {
        let buf = BufSlice::from_bytes(vec![0, 1, 2, 3, 4, 5]);
        let mut rdr = BufRdr::from_slice(buf);
        let frag0 = rdr.take_precisely(1).unwrap();
        assert_eq!(frag0.data(), vec![0]);
        let frag12 = rdr.take_precisely(2).unwrap();
        assert_eq!(frag12.data(), vec![1, 2]);
        assert!(!rdr.is_empty());
        let frag_err = rdr.take_precisely(99);
        assert!(frag_err.is_err());
        let frag345 = rdr.take_precisely(3).unwrap();
        assert_eq!(frag345.data(), vec![3, 4, 5]);
        assert!(rdr.is_empty());
        let frag_err = rdr.take_precisely(1);
        assert!(frag_err.is_err());
    }
}
