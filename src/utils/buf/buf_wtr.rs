#[derive(Debug)]
pub enum Error {
    NotEnoughSpace,
}

pub trait BufWtr {
    fn data_len(&self) -> usize;
    fn front_len(&self) -> usize;
    fn back_len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn is_full(&self) -> bool;
    fn data(&self) -> &[u8];
    fn data_mut(&mut self) -> &mut [u8];
    fn front_free_space(&mut self) -> &mut [u8];
    fn back_free_space(&mut self) -> &mut [u8];
    fn grow_front(&mut self, len: usize) -> Result<(), Error>;
    fn grow_back(&mut self, len: usize) -> Result<(), Error>;
    fn shrink_front(&mut self, len: usize) -> Result<(), Error>;
    fn shrink_back(&mut self, len: usize) -> Result<(), Error>;
    fn reset_data(&mut self, start: usize);
    fn append(&mut self, n: &[u8]) -> Result<(), Error>;
    fn prepend(&mut self, n: &[u8]) -> Result<(), Error>;
}
