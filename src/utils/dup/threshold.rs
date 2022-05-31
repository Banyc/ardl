use super::ConsecutiveDuplicateCount;

pub struct DuplicateThreshold<T>
where
    T: PartialEq,
{
    duplicate: ConsecutiveDuplicateCount<T>,
    dup_threshold_to_activate: usize, // activation: inclusive
}

impl<T> DuplicateThreshold<T>
where
    T: PartialEq,
{
    pub fn new(default_value: T, dup_limit_to_activate: usize) -> Self {
        DuplicateThreshold {
            duplicate: ConsecutiveDuplicateCount::new(default_value),
            dup_threshold_to_activate: dup_limit_to_activate,
        }
    }

    pub fn is_activated(&self) -> bool {
        self.dup_threshold_to_activate <= self.duplicate.count()
    }

    pub fn recount(&mut self) {
        self.duplicate.recount();
    }

    pub fn set(&mut self, v: T) {
        self.duplicate.set(v);
    }
}
