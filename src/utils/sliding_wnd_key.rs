pub trait SlidingWndKey {
    fn add_usize(&self, n: usize) -> Self;
    fn sub(&self, other: &Self) -> usize;
}
