use super::{BufWtr, BufRdr};

pub enum BufAny {
    Reader(BufRdr),
    Writer(BufWtr),
}
