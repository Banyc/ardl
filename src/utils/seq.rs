use std::{cmp::Ordering, num::Wrapping};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Seq {
    n: u32,
}

impl Seq {
    pub fn from_u32(n: u32) -> Self {
        Seq { n }
    }

    pub fn to_u32(&self) -> u32 {
        self.n
    }

    pub fn add_u32(&self, n: u32) -> Seq {
        let s = Wrapping(self.n) + Wrapping(n);
        Seq { n: s.0 }
    }

    pub fn sub_seq(&self, other: Seq) -> u32 {
        let s = Wrapping(self.n) - Wrapping(other.n);
        s.0
    }

    pub fn increment(&mut self) {
        *self = self.add_u32(1);
    }

    pub fn max(lhs: Seq, rhs: Seq) -> Seq {
        if lhs < rhs {
            rhs
        } else {
            lhs
        }
    }
}

impl PartialOrd for Seq {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Seq {
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
    use super::Seq;

    #[test]
    fn cmp_wraparound() {
        let a = Seq::from_u32(u32::MAX);
        let b = Seq::from_u32(u32::MIN);
        assert!(a < b);
    }

    #[test]
    fn cmp_wo_wraparound() {
        let a = Seq::from_u32(0);
        let b = Seq::from_u32(1);
        assert!(a < b);
    }

    #[test]
    fn cmp_far() {
        let a = Seq::from_u32(0);
        let b = Seq::from_u32(i32::MAX as u32);
        let c = Seq::from_u32(i32::MAX as u32 + 1);
        assert!(a < b);
        assert!(c < a);
    }

    #[test]
    fn add_wraparound() {
        let a = Seq::from_u32(u32::MAX);
        let b = a.add_u32(1);
        assert_eq!(b.to_u32(), 0);
    }

    #[test]
    fn add_wo_wraparound() {
        let a = Seq::from_u32(0);
        let b = a.add_u32(1);
        assert_eq!(b.to_u32(), 1);
    }

    #[test]
    fn increment_wo_wraparound() {
        let mut a = Seq::from_u32(0);
        a.increment();
        assert_eq!(a.to_u32(), 1);
    }

    #[test]
    fn sub_wraparound() {
        let a = Seq::from_u32(0);
        let b = Seq::from_u32(u32::MAX);
        assert_eq!(a.sub_seq(b), 1);
    }
}
