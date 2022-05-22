use super::{BufRdr, BufWtr};

pub enum BufAny {
    Reader(BufRdr),
    Writer(BufWtr),
}
