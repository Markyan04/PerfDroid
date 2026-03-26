use std::sync::atomic::Ordering;

use crate::errors::CoreError;
use crate::types::{CollectorMetadata, ControlCommand, MetricBatch, ProfilerMetadata};

/// Trait describing atomic metric semantics shared by profiler collectors.
pub trait AtomicMetric: Send + Sync {
    /// Marks the underlying metric value as unavailable.
    fn mark_missing(&self, ordering: Ordering);

    /// Returns whether the underlying metric value is unavailable.
    fn is_missing(&self, ordering: Ordering) -> bool;
}

/// Trait representing a single collector in a profiler.
pub trait Collector: Send + Sync {
    /// Returns static metadata describing this collector.
    fn metadata(&self) -> &CollectorMetadata;

    /// Reads collector's latest buffered value.
    fn read_buffer(&self, ordering: Ordering) -> i64;
}

/// Trait representing a profiler made of one or more collectors.
pub trait Profiler: Send + Sync {
    /// Collector type used by this profiler.
    type CollectorType: Collector;

    /// Returns profiler-level metadata.
    fn metadata(&self) -> &ProfilerMetadata;

    /// Returns all collectors owned by this profiler.
    fn collectors(&self) -> &[Self::CollectorType];

    fn connect(&mut self) -> Result<(), CoreError>;

    fn start(&mut self) -> Result<(), CoreError>;

    fn pause(&mut self) -> Result<(), CoreError>;

    fn restart(&mut self) -> Result<(), CoreError>;

    fn stop(&mut self) -> Result<(), CoreError>;
}

/// Data-plane abstraction that assembles outgoing metric batches.
pub trait DataPlane {
    /// Builds one metric batch from current profiler-side state.
    fn build_metric_batch(&self) -> Result<MetricBatch, CoreError>;
}

/// Control-plane abstraction for command-driven runtime control.
pub trait ControlPlane {
    /// Applies a runtime control command.
    fn apply_control<P: Profiler>(
        &mut self,
        command: ControlCommand,
        target: P,
    ) -> Result<(), CoreError>;
}
