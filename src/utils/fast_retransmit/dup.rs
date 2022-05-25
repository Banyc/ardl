pub struct DuplicateCount<T>
where
    T: PartialEq,
{
    value: T,
    count: usize,
}

impl<T> DuplicateCount<T>
where
    T: PartialEq,
{
    pub fn new(value: T) -> Self {
        let this = DuplicateCount::<T> { value, count: 0 };
        this
    }

    pub fn set(&mut self, v: T) {
        if v == self.value {
            self.count += 1;
        }
        self.value = v;
    }

    pub fn count(&self) -> usize {
        self.count
    }
}
