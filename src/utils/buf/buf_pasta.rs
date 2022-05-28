use super::{BufSlice, BufWtr, OwnedBufWtr};

pub struct BufPasta {
    slices: Vec<BufSlice>,
    len: usize,
}

#[derive(Debug)]
pub enum Error {
    NotEnoughSpace,
}

impl BufPasta {
    #[inline]
    fn check_rep(&self) {
        let mut cum_len = 0;
        for frag in &self.slices {
            cum_len += frag.data().len();
        }
        assert_eq!(cum_len, self.len);
    }

    pub fn new() -> Self {
        let this = BufPasta {
            slices: Vec::new(),
            len: 0,
        };
        this.check_rep();
        this
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn append(&mut self, frag: BufSlice) {
        self.len += frag.data().len();
        self.slices.push(frag);
        self.check_rep();
    }

    pub fn append_to(&self, wtr: &mut impl BufWtr) -> Result<(), Error> {
        if wtr.back_len() < self.len {
            return Err(Error::NotEnoughSpace);
        }
        for frag in &self.slices {
            wtr.append(frag.data()).unwrap();
        }
        Ok(())
    }

    pub fn prepend_to(&self, wtr: &mut OwnedBufWtr) -> Result<(), Error> {
        if wtr.front_len() < self.len {
            return Err(Error::NotEnoughSpace);
        }
        for frag in &self.slices {
            wtr.prepend(frag.data()).unwrap();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::buf::BufSlice;

    use super::{BufWtr, OwnedBufWtr};

    use super::BufPasta;

    #[test]
    fn append() {
        let mut wtr = OwnedBufWtr::new(1024, 512);
        let mut pasta = BufPasta::new();
        let origin1 = vec![0, 1, 2, 3];
        let mut wtr1 = OwnedBufWtr::new(1024, 512);
        wtr1.append(&origin1).unwrap();
        let origin2 = vec![4];
        let mut wtr2 = OwnedBufWtr::new(1024, 512);
        wtr2.append(&origin2).unwrap();
        let slice1 = BufSlice::from_wtr(wtr1);
        let slice2 = BufSlice::from_wtr(wtr2);

        assert_eq!(pasta.len(), 0);
        pasta.append(slice1.slice(0..2).unwrap());
        assert_eq!(pasta.len(), 2);
        pasta.append(slice2.slice(0..1).unwrap());
        assert_eq!(pasta.len(), 3);
        pasta.append(slice1.slice(2..4).unwrap());
        assert_eq!(pasta.len(), 5);

        pasta.append_to(&mut wtr).unwrap();
        assert_eq!(wtr.data(), vec![0, 1, 4, 2, 3]);

        assert_eq!(pasta.len(), 5);
    }
}
