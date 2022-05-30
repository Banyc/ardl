use std::fmt::Debug;

pub trait Seq: PartialOrd + Ord + Copy + Debug {
    fn add_usize(&self, n: usize) -> Self;
    fn sub(&self, other: &Self) -> usize;
    fn zero() -> Self;
}
