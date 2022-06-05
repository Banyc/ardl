use super::BufSlice;
use std::collections::VecDeque;

pub struct BufSlicerQue {
    queue: VecDeque<BufSlice>,
    len_cap: usize,
}

impl BufSlicerQue {
    fn check_rep(&self) {
        assert!(self.queue.len() <= self.len_cap);
        for slice in &self.queue {
            assert!(!slice.is_empty());
        }
    }

    pub fn new(len_cap: usize) -> Self {
        let this = BufSlicerQue {
            queue: VecDeque::new(),
            len_cap,
        };
        this.check_rep();
        this
    }

    pub fn push_back(&mut self, slice: BufSlice) -> Result<(), PushError<BufSlice>> {
        if self.is_full() {
            return Err(PushError(slice));
        }
        if slice.is_empty() {
            return Ok(());
        }

        self.queue.push_back(slice);
        self.check_rep();
        Ok(())
    }

    pub fn slice_front(&mut self, max_len: usize) -> Result<BufSlice, Error> {
        let slice = match self.queue.pop_front() {
            Some(x) => x,
            None => return Err(Error::NothingToSlice),
        };
        if slice.len() <= max_len {
            self.check_rep();
            Ok(slice)
        } else {
            let mut slice = slice;
            let front = slice.pop_front(max_len).unwrap();
            self.queue.push_front(slice);
            self.check_rep();
            Ok(front)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.queue.len() == self.len_cap
    }
}

#[derive(Debug)]
pub enum Error {
    NothingToSlice,
}

pub struct PushError<T>(pub T);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test1() {
        let mut slicer = BufSlicerQue::new(2);

        let slice1 = BufSlice::from_bytes(vec![0]);
        let slice2 = BufSlice::from_bytes(vec![1, 2]);

        slicer.push_back(slice1).map_err(|_| ()).unwrap();
        slicer.push_back(slice2).map_err(|_| ()).unwrap();
        assert!(slicer.is_full());

        let slice1 = slicer.slice_front(2).unwrap();
        assert_eq!(slice1.data(), vec![0]);

        let slice2 = slicer.slice_front(1).unwrap();
        assert_eq!(slice2.data(), vec![1]);

        let slice3 = slicer.slice_front(2).unwrap();
        assert_eq!(slice3.data(), vec![2]);

        assert!(slicer.is_empty());
    }
}
