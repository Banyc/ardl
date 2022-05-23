use super::{BufRdr, OwnedBufWtr};

pub enum BufAny {
    Reader(BufRdr),
    Writer(OwnedBufWtr),
}
