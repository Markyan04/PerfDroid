use std::sync::atomic::{AtomicI64, Ordering};

use crate::constants::INVALID_METRIC_VALUE;

/// Compatibility wrapper around atomic metric storage.
///
/// New implementations should prefer implementing [`crate::traits::AtomicMetric`]
/// on their own concrete type.
#[deprecated(note = "Duplicate of crate::traits::AtomicMetric; implement that instead")]
#[derive(Debug)]
pub struct AtomicMetricValue {
    inner: AtomicI64,
}

impl AtomicMetricValue {
    /// Creates a new atomic value with an initial metric value.
    pub const fn new(initial: i64) -> Self {
        Self {
            inner: AtomicI64::new(initial),
        }
    }

    /// Loads current metric value using the provided memory ordering.
    pub fn load(&self, ordering: Ordering) -> i64 {
        self.inner.load(ordering)
    }

    /// Stores metric value using the provided memory ordering.
    pub fn store(&self, value: i64, ordering: Ordering) {
        self.inner.store(value, ordering);
    }

    /// Swaps metric value and returns the previous one.
    pub fn swap(&self, value: i64, ordering: Ordering) -> i64 {
        self.inner.swap(value, ordering)
    }

    /// Marks this metric as missing.
    pub fn mark_missing(&self, ordering: Ordering) {
        self.store(INVALID_METRIC_VALUE, ordering);
    }

    /// Returns true if current value is the missing sentinel.
    pub fn is_missing(&self, ordering: Ordering) -> bool {
        self.load(ordering) == INVALID_METRIC_VALUE
    }
}

impl Default for AtomicMetricValue {
    /// Creates a metric initialized to [`INVALID_METRIC_VALUE`].
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
