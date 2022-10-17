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
        for slice in &self.slices {
            cum_len += slice.data().len();
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

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn append(&mut self, slice: BufSlice) {
        self.len += slice.data().len();
        self.slices.push(slice);
        self.check_rep();
    }

    pub fn append_to(&self, wtr: &mut impl BufWtr) -> Result<(), Error> {
        if wtr.back_len() < self.len {
            return Err(Error::NotEnoughSpace);
        }
        for slice in &self.slices {
            wtr.append(slice.data()).unwrap();
        }
        Ok(())
    }

    pub fn prepend_to(&self, wtr: &mut OwnedBufWtr) -> Result<(), Error> {
        if wtr.front_len() < self.len {
            return Err(Error::NotEnoughSpace);
        }
        for slice in &self.slices {
            wtr.prepend(slice.data()).unwrap();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::{BufPasta, BufSlice, BufWtr, OwnedBufWtr};

    #[test]
    fn append() {
        let mut wtr = OwnedBufWtr::new(1024, 512);
        let mut pasta = BufPasta::new();
        let origin1 = vec![0, 1, 2, 3];
        let origin2 = vec![4];
        let slice1 = BufSlice::from_bytes(origin1);
        let slice2 = BufSlice::from_bytes(origin2);

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
