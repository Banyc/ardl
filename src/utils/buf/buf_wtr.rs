#[derive(Debug)]
pub struct BufWtr {
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

#[derive(Debug)]
pub enum Error {
    NotEnoughSpace,
}

impl BufWtr {
    #[inline]
    pub fn check_rep(&self) {
        assert!(self.start <= self.end);
        assert!(self.end <= self.buf.len());
    }
    pub fn from_vec(buf: Vec<u8>, start: usize, end: usize) -> Self {
        let this = Self { buf, start, end };
        this.check_rep();
        this
    }
    pub fn new(len: usize, start: usize) -> Self {
        let this = Self {
            buf: vec![0; len],
            start,
            end: start,
        };
        this.check_rep();
        this
    }
    #[inline]
    pub fn data_len(&self) -> usize {
        self.end - self.start
    }
    #[inline]
    pub fn front_len(&self) -> usize {
        self.start
    }
    #[inline]
    pub fn back_len(&self) -> usize {
        self.buf.len() - self.end
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data_len() == 0
    }
    pub fn is_full(&self) -> bool {
        self.data_len() == self.buf.len()
    }
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }
    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.start..self.end]
    }
    #[inline]
    pub fn front_free_space(&mut self) -> &mut [u8] {
        &mut self.buf[..self.start]
    }
    #[inline]
    pub fn back_free_space(&mut self) -> &mut [u8] {
        &mut self.buf[self.end..]
    }
    #[inline]
    pub fn grow_front(&mut self, len: usize) -> Result<(), Error> {
        if self.start < len {
            return Err(Error::NotEnoughSpace);
        }
        self.start -= len;
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn grow_back(&mut self, len: usize) -> Result<(), Error> {
        if self.buf.len() < self.end + len {
            return Err(Error::NotEnoughSpace);
        }
        self.end += len;
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn shrink_front(&mut self, len: usize) -> Result<(), Error> {
        if self.end < self.start + len {
            return Err(Error::NotEnoughSpace);
        }
        self.start += len;
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn shrink_back(&mut self, len: usize) -> Result<(), Error> {
        if self.end < self.start + len {
            return Err(Error::NotEnoughSpace);
        }
        self.end -= len;
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn reset_data(&mut self, start: usize) {
        self.start = start;
        self.end = start;
        self.check_rep();
    }
    #[inline]
    pub fn append(&mut self, n: &[u8]) -> Result<(), Error> {
        if self.back_free_space().len() < n.len() {
            return Err(Error::NotEnoughSpace);
        }
        self.back_free_space()[..n.len()].copy_from_slice(n);
        self.grow_back(n.len()).unwrap();
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn prepend(&mut self, n: &[u8]) -> Result<(), Error> {
        if self.front_free_space().len() < n.len() {
            return Err(Error::NotEnoughSpace);
        }
        let start = self.front_free_space().len() - n.len();
        self.front_free_space()[start..].copy_from_slice(n);
        self.grow_front(n.len()).unwrap();
        self.check_rep();
        Ok(())
    }
    #[inline]
    pub fn assign(&mut self, other: BufWtr) {
        self.buf = other.buf;
        self.start = other.start;
        self.end = other.end;
        self.check_rep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy() {
        let mut buf = BufWtr::new(1024, 512);
        let tail = vec![1, 2, 3];
        let head = vec![4, 5, 6];
        buf.append(&tail).unwrap();
        buf.prepend(&head).unwrap();
        assert_eq!(buf.data(), vec![4, 5, 6, 1, 2, 3]);
    }
}
