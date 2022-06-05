use crate::utils::Seq;
use std::{cmp::Ordering, num::Wrapping};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct Seq32 {
    n: u32,
}

impl Seq32 {
    pub fn from_u32(n: u32) -> Self {
        Seq32 { n }
    }

    pub fn to_u32(&self) -> u32 {
        self.n
    }

    pub fn increment(&mut self) {
        *self = self.add_usize(1);
    }

    pub fn max(lhs: Seq32, rhs: Seq32) -> Seq32 {
        if lhs < rhs {
            rhs
        } else {
            lhs
        }
    }
}

impl Seq for Seq32 {
    fn add_usize(&self, n: usize) -> Self {
        let s = Wrapping(self.n) + Wrapping(n as u32);
        Seq32 { n: s.0 }
    }

    fn sub(&self, other: &Self) -> usize {
        let s = Wrapping(self.n) - Wrapping(other.n);
        s.0 as usize
    }

    fn zero() -> Self {
        Seq32::from_u32(0)
    }
}

impl PartialOrd for Seq32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Seq32 {
    fn cmp(&self, other: &Self) -> Ordering {
        let ord = match self.n.partial_cmp(&other.n).unwrap() {
            Ordering::Less => {
                let diff = other.n - self.n;
                match diff <= u32::MAX / 2 {
                    true => Ordering::Less,
                    false => Ordering::Greater,
                }
            }
            Ordering::Equal => Ordering::Equal,
            Ordering::Greater => {
                let diff = self.n - other.n;
                match diff <= u32::MAX / 2 {
                    true => Ordering::Greater,
                    false => Ordering::Less,
                }
            }
        };
        ord
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::Seq;

    use super::Seq32;

    #[test]
    fn cmp_wraparound() {
        let a = Seq32::from_u32(u32::MAX);
        let b = Seq32::from_u32(u32::MIN);
        assert!(a < b);
    }

    #[test]
    fn cmp_wo_wraparound() {
        let a = Seq32::from_u32(0);
        let b = Seq32::from_u32(1);
        assert!(a < b);
    }

    #[test]
    fn cmp_far() {
        let a = Seq32::from_u32(0);
        let b = Seq32::from_u32(i32::MAX as u32);
        let c = Seq32::from_u32(i32::MAX as u32 + 1);
        assert!(a < b);
        assert!(c < a);
    }

    #[test]
    fn add_wraparound() {
        let a = Seq32::from_u32(u32::MAX);
        let b = a.add_usize(1);
        assert_eq!(b.to_u32(), 0);
    }

    #[test]
    fn add_wo_wraparound() {
        let a = Seq32::from_u32(0);
        let b = a.add_usize(1);
        assert_eq!(b.to_u32(), 1);
    }

    #[test]
    fn increment_wo_wraparound() {
        let mut a = Seq32::from_u32(0);
        a.increment();
        assert_eq!(a.to_u32(), 1);
    }

    #[test]
    fn sub_wraparound() {
        let a = Seq32::from_u32(0);
        let b = Seq32::from_u32(u32::MAX);
        assert_eq!(a.sub(&b), 1);
    }

    #[test]
    fn sub_zero() {
        let a = Seq32::from_u32(1);
        let b = Seq32::from_u32(1);
        assert_eq!(a.sub(&b), 0);
    }

    #[test]
    fn sub_wo_wraparound() {
        let a = Seq32::from_u32(3);
        let b = Seq32::from_u32(1);
        assert_eq!(a.sub(&b), 2);
    }
}
