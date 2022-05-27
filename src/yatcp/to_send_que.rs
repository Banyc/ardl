use std::collections::VecDeque;

use crate::utils;

use super::SendError;

pub struct ToSendQue {
    queue: VecDeque<utils::BufRdr>,
    byte_len: usize,
    byte_capacity: usize,
}

impl ToSendQue {
    fn check_rep(&self) {
        assert!(self.byte_len <= self.byte_capacity);
        if !self.queue.is_empty() {
            assert!(!self.queue.front().unwrap().is_empty());
        }
    }

    pub fn new(byte_capacity: usize) -> Self {
        let this = ToSendQue {
            queue: VecDeque::new(),
            byte_len: 0,
            byte_capacity,
        };
        this.check_rep();
        this
    }

    pub fn push_back(&mut self, rdr: utils::BufRdr) -> Result<(), SendError<utils::BufRdr>> {
        let rdr_len = rdr.len();
        if !(self.byte_len + rdr_len <= self.byte_capacity) {
            return Err(SendError(rdr));
        }
        if rdr.is_empty() {
            return Ok(());
        }

        self.byte_len += rdr_len;
        self.queue.push_back(rdr);
        self.check_rep();
        Ok(())
    }

    pub fn slice_front(&mut self, max_len: usize) -> utils::BufFrag {
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
