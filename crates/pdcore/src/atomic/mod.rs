use std::sync::atomic::{AtomicI64, Ordering};

use crate::constants::INVALID_METRIC_VALUE;

/// Duplicate of [`crate::traits::AtomicMetric`].
/// Design a struct and implement [`crate::traits::AtomicMetric`] instead.
#[deprecated(note = "Duplicate of crate::traits::AtomicMetric; implement that instead")]
#[derive(Debug)]
pub struct AtomicMetricValue {
    inner: AtomicI64,
}

impl AtomicMetricValue {
    pub const fn new(initial: i64) -> Self {
        Self {
            inner: AtomicI64::new(initial),
        }
    }

    pub fn load(&self, ordering: Ordering) -> i64 {
        self.inner.load(ordering)
    }

    pub fn store(&self, value: i64, ordering: Ordering) {
        self.inner.store(value, ordering);
    }

    pub fn swap(&self, value: i64, ordering: Ordering) -> i64 {
        self.inner.swap(value, ordering)
    }

    pub fn mark_missing(&self, ordering: Ordering) {
        self.store(INVALID_METRIC_VALUE, ordering);
    }

    pub fn is_missing(&self, ordering: Ordering) -> bool {
        self.load(ordering) == INVALID_METRIC_VALUE
    }
}

impl Default for AtomicMetricValue {
    fn default() -> Self {
        Self::new(INVALID_METRIC_VALUE)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::AtomicMetricValue;
    use crate::constants::INVALID_METRIC_VALUE;

    #[test]
    fn default_uses_missing_sentinel() {
        let value = AtomicMetricValue::default();
        assert_eq!(value.load(Ordering::Relaxed), INVALID_METRIC_VALUE);
    }

    #[test]
    fn store_and_load_roundtrip() {
        let value = AtomicMetricValue::default();
        value.store(118, Ordering::Release);
        assert_eq!(value.load(Ordering::Acquire), 118);
    }
}
