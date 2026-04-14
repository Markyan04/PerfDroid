//! Core abstractions for PerfDroid.
//!
//! `pdcore` contains shared types, traits, constants, and validation logic
//! used across profiler, registry, and app layers.

/// Shared constants used by metric collection and aggregation.
pub mod constants;
/// Error definitions shared by constructors and validators.
pub mod errors;
/// Core traits used to model profiler/data-plane/control-plane behavior.
pub mod traits;
/// Core domain types used across crate boundaries.
pub mod types;
/// Utility helpers for validation and value normalization.
pub mod utils;

/// Sentinel value representing missing/disabled/unavailable metric data.
pub use constants::INVALID_METRIC_VALUE;
/// Fixed metric vector width used in [`types::MetricBatch`].
pub use constants::METRIC_VALUES_CAPACITY;
/// Shared error type returned by this crate.
pub use errors::CoreError;
