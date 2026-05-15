/// Sentinel value used when a collector is disabled, unavailable,
/// or the data point is lost.
pub const INVALID_METRIC_VALUE: i64 = -1;

/// Standard fixed length for [`crate::types::MetricBatch::values`].
///
/// Values shorter than this are padded with [`INVALID_METRIC_VALUE`].
pub const METRIC_VALUES_CAPACITY: usize = 10;
