pub struct ConsecutiveDuplicateCount<T>
where
    T: PartialEq,
{
    value: T,
    count: usize,
}

impl<T> ConsecutiveDuplicateCount<T>
where
    T: PartialEq,
{
    pub fn new(value: T) -> Self {
        let this = ConsecutiveDuplicateCount::<T> { value, count: 0 };
        this
    }

    pub fn set(&mut self, v: T) {
        if v == self.value {
            self.count += 1;
        } else {
            self.count = 0;
        }
        self.value = v;
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::ConsecutiveDuplicateCount;

    #[test]
    fn empty_count() {
        let dup = ConsecutiveDuplicateCount::new(9);
        assert_eq!(dup.count(), 0);
    }

    #[test]
    fn set_same_count() {
        let mut dup = ConsecutiveDuplicateCount::new(9);
        dup.set(9);
        assert_eq!(dup.count(), 1);
        dup.set(9);
        assert_eq!(dup.count(), 2);
    }

    #[test]
    fn set_diff_count() {
        let mut dup = ConsecutiveDuplicateCount::new(9);
        dup.set(9);
        assert_eq!(dup.count(), 1);
        dup.set(8);
        assert_eq!(dup.count(), 0);
        dup.set(8);
        assert_eq!(dup.count(), 1);
    }
}
