use std::{ops::Range, rc::Rc};

use super::{BufWtr, BufWtrTrait};

pub struct BufFrag {
    buf: Rc<BufWtr>,
    range: Range<usize>,
}

pub struct BufFragBuilder {
    pub buf: Rc<BufWtr>,
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
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::utils::{BufWtr, BufWtrTrait};

    use super::BufFragBuilder;

    #[test]
    fn frag() {
        let mut buf = BufWtr::new(1024, 512);
        buf.append(&vec![0, 1, 2]).unwrap();
        let frag = BufFragBuilder {
            buf: Rc::new(buf),
            range: 1..3,
        }
        .build();
        assert_eq!(frag.data(), &vec![1, 2]);
    }
}
